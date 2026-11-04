//! streamrip (encore_requests::sources::streamrip) on a server's settings, with a stand-in for its `rip`
//! program: its config written, checked and reset, and its failures in plain words; and a Deezer album
//! picked in the picker, downloaded by the stand-in and imported by the request worker, end to end
//! (Deezer itself is out of reach here: `deezer_album_live` runs the real streamrip when asked to).
use crate::state::AppState;
use encore_core::fsx;
use encore_db::{p, RowExt};
use encore_http::Ctx as _;
use encore_requests::sources::streamrip::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn state(name: &str) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("encore-rip-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::test_support::with_sources(&st);
    (st, dir)
}

/// A stand-in for streamrip's `rip`, in `dir`: `config path` writes the 2.x default file when there is
/// none and complains the way the fork does about a file marked `broken = true`; `url` does what the
/// `mode` file beside it says (prompt for the ARL, fail to load the config, download one song, download the
/// FLACs in `fixture/` as an album, fail with output and exit 1, or `hold`: note the run in `runs.log` – and
/// in `overlap.log` when another run is under way –, then wait for a `release` file, which it takes, or for
/// `release-all`, and download one song). Every call, with what its stdin is, lands in `calls.log`; a `url`
/// run first writes to and reads from /dev/tty (which must neither hang nor fail the run), and leaves its
/// /proc stat and environment in `stat` and `env`.
pub(super) fn fake_rip(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let body = r#"here=$(dirname "$0")
echo "call: $* stdin=$(readlink /proc/$$/fd/0 2>/dev/null || echo unknown)" >> "$here/calls.log"
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
  echo "No file found at $cfg, creating default config."
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
# no_update_check = true

[misc]
version = "2.0.6"
EOF
fi
if grep -q '^broken = true' "$cfg"; then
  echo "Error loading config from $cfg: "
  echo "QobuzConfig.__init__() got an unexpected keyword argument 'user_id'"
  echo "Try running rip config reset"
  exit 0
fi
case "$sub" in
  path) echo "$cfg";;
  url)
cat /proc/$$/stat > "$here/stat"
echo "TERM=$TERM NO_COLOR=$NO_COLOR COLUMNS=$COLUMNS PYTHONUNBUFFERED=$PYTHONUNBUFFERED HOME=$HOME" > "$here/env"
echo "Logging in to Deezer" > /dev/tty
read answer < /dev/tty
mode=$(cat "$here/mode" 2>/dev/null)
case "$mode" in
  prompt)
    echo "If you're not sure how to find the ARL cookie, see the instructions at https://github.com/nathom/streamrip/wiki/Finding-Your-Deezer-ARL-Cookie"
    echo "Enter your ARL: Traceback (most recent call last):"
    echo "  File \"rip/prompter.py\", line 80, in prompt_creds"
    echo "EOFError: EOF when reading a line"
    exit 1;;
  config_error)
    echo "Error loading config from $cfg: "
    echo "Need to update config from 2.0.5 to 2.0.6"
    echo "Try running rip config reset"
    exit 0;;
  album)
    echo "Downloading Illuminate EP by Duumu"
    mkdir -p "$folder/Duumu - Illuminate EP"
    cp "$here"/fixture/*.flac "$folder/Duumu - Illuminate EP/"
    echo "WARNING: a warning on stderr" >&2
    echo "Downloaded 4 tracks";;
  errno)
    mkdir -p "$folder/Duumu - Illuminate EP"
    echo "Downloading Illuminate EP by Duumu"
    echo "OSError: [Errno 5] Input/output error: '$folder/Duumu - Illuminate EP/01. Light 1.flac'" >&2
    exit 1;;
  fail)
    echo "Resolving https://www.deezer.com/album/302127"
    echo "first line of the run, kept in the log"
    echo "Traceback (most recent call last):" >&2
    echo "  File \"rip/main.py\", line 120, in rip" >&2
    echo "NonStreamableError: This album is not available in your country" >&2
    exit 1;;
  hold)
    echo "start $url" >> "$here/runs.log"
    if ls "$here"/running.* >/dev/null 2>&1; then echo "overlap $url" >> "$here/overlap.log"; fi
    touch "$here/running.$$"
    while [ ! -f "$here/release" ] && [ ! -f "$here/release-all" ]; do sleep 0.05; done
    rm -f "$here/release" "$here/running.$$"
    mkdir -p "$folder/Album"; printf 'fLaC' > "$folder/Album/01 Song.flac";;
  *) mkdir -p "$folder/Album"; printf 'fLaC' > "$folder/Album/01 Song.flac";;
esac;;
esac
"#;
    let p = dir.join("rip");
    let tmp = dir.join(".rip.tmp");
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

#[tokio::test]
async fn a_missing_or_broken_config_is_written_fresh_with_the_sign_ins_carried_over() {
    let (st, dir) = state("config");
    let rip = fake_rip(&dir.join("fake"));
    st.cfg().set_many(&serde_json::json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_qobuz_user": "qu", "streamrip_qobuz_token": "qt" }));
    assert_eq!(binary(&st).as_deref(), Some(rip.to_string_lossy().as_ref()));
    // no file yet: streamrip writes its defaults, Encore its credentials (2.x key names, downloads db off)
    let c = check_config(&st, false).await.unwrap();
    assert!(c.regenerated && c.backup.is_none() && c.problem.is_none(), "{c:?}");
    let text = std::fs::read_to_string(&c.path).unwrap();
    assert_eq!(toml_value(&text, "qobuz", "email_or_userid"), "qu");
    assert_eq!(toml_value(&text, "qobuz", "password_or_token"), "qt");
    assert_eq!(toml_value(&text, "qobuz", "use_auth_token"), "true");
    assert!(!section_body(&text, "qobuz").contains("user_id ="), "{text}");
    assert_eq!(toml_value(&text, "database", "downloads_enabled"), "false");
    assert_eq!(toml_value(&text, "misc", "version"), "2.0.6");
    // a sound file is left alone, and Diagnostics says so
    let c = check_config(&st, false).await.unwrap();
    assert!(!c.regenerated && c.problem.is_none(), "{c:?}");
    let check = config_loads_check(&st).await;
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["name"], "streamrip config loads");
    // the file breaks (keys from another streamrip, say) while holding a Tidal sign-in, a hand-typed ARL
    // and the 1.x Qobuz login
    let mut broken = text.clone();
    for (k, v) in [("access_token", "\"tok-default-abcdef\""), ("hires_access_token", "\"tok-hires-abcdef\""), ("hires_client", "true")] {
        broken = set_toml(&broken, "tidal", k, v);
    }
    broken = set_toml(&broken, "deezer", "arl", "\"old-arl-cookie\"");
    broken = set_toml(&broken, "qobuz", "email_or_userid", "\"\"");
    broken = set_toml(&broken, "qobuz", "password_or_token", "\"\"");
    broken = set_toml(&broken, "qobuz", "user_id", "\"polluted\"");
    broken = set_toml(&broken, "qobuz", "auth_token", "\"polluted-token\"");
    broken = set_toml(&broken, "misc", "broken", "true");
    std::fs::write(&c.path, &broken).unwrap();
    st.cfg().set_many(&serde_json::json!({ "streamrip_qobuz_user": "", "streamrip_qobuz_token": "" }));
    let check = config_loads_check(&st).await;
    assert_eq!(check["ok"], false, "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(detail.contains("unexpected keyword argument 'user_id'") && detail.contains("Reset streamrip config"), "{detail}");
    let c = check_config(&st, false).await.unwrap();
    assert!(c.regenerated && c.problem.is_none(), "{c:?}");
    let backup = c.backup.clone().expect("the old file is kept");
    assert!(backup.file_name().unwrap().to_string_lossy().starts_with("config.toml.bak-"), "{backup:?}");
    assert!(std::fs::read_to_string(&backup).unwrap().contains("broken = true"));
    let fresh = std::fs::read_to_string(&c.path).unwrap();
    assert!(!fresh.contains("broken"), "{fresh}");
    let t = parse_tidal_toml(&fresh);
    assert_eq!(t["access_token"], "tok-default-abcdef");
    assert_eq!(t["hires_access_token"], "tok-hires-abcdef");
    assert_eq!(t["hires_client"], "true");
    assert_eq!(toml_value(&fresh, "deezer", "arl"), "old-arl-cookie");
    assert_eq!(toml_value(&fresh, "qobuz", "email_or_userid"), "polluted", "the 1.x login moves under the 2.x key names");
    assert_eq!(toml_value(&fresh, "qobuz", "password_or_token"), "polluted-token");
    assert_eq!(toml_value(&fresh, "qobuz", "use_auth_token"), "true");
    assert!(!fresh.lines().any(|l| l.starts_with("user_id = \"polluted") || l.starts_with("auth_token =")), "{fresh}");
    assert!(c.message().contains("carried over"), "{}", c.message());
    // "Reset streamrip config" on a sound file: a fresh one all the same, sign-ins kept
    let c = reset_config(&st).await.unwrap();
    assert!(c.regenerated && c.backup.is_some() && c.problem.is_none(), "{c:?}");
    assert_eq!(parse_tidal_toml(&std::fs::read_to_string(&c.path).unwrap())["access_token"], "tok-default-abcdef");
    assert_eq!(c.json()["ok"], true);
    // a run whose output is the fork's config complaint: plain words, the whole output for the log
    std::fs::write(dir.join("fake/mode"), "config_error").unwrap();
    let e = download(&st, "https://tidal.com/browse/track/1", &dir.join("out")).await.expect_err("no files");
    let m = e.to_string();
    assert!(m.starts_with(CONFIG_BROKEN_MSG), "{m}");
    assert!(m.contains("Need to update config from 2.0.5 to 2.0.6"), "{m}");
    assert!(run_log(&e).unwrap().contains("Try running rip config reset"));
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(calls.lines().last().unwrap().contains("url https://tidal.com/browse/track/1"), "{calls}");
    assert!(calls.lines().all(|l| l.contains("stdin=/dev/null")), "stdin is closed on every run:\n{calls}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_deezer_arl_prompt_becomes_a_plain_message() {
    let (st, dir) = state("arl");
    let rip = fake_rip(&dir.join("fake"));
    st.cfg().set_many(&serde_json::json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy() }));
    let out = dir.join("out");
    // no ARL anywhere: a Deezer link is refused before streamrip runs
    let e = download(&st, "https://www.deezer.com/track/1", &out).await.expect_err("no ARL");
    assert_eq!(e.to_string(), ARL_MSG);
    assert!(run_log(&e).is_none());
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(!calls.contains(" url "), "{calls}");
    // an ARL streamrip still asks about (its prompt reads EOF): the same plain message, the output kept
    st.cfg().set_many(&serde_json::json!({ "streamrip_deezer_arl": "a-cookie" }));
    std::fs::write(dir.join("fake/mode"), "prompt").unwrap();
    let e = download(&st, "https://www.deezer.com/track/1", &out).await.expect_err("prompted");
    assert_eq!(e.to_string(), ARL_MSG);
    assert!(run_log(&e).unwrap().contains("EOFError: EOF when reading a line"));
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(calls.contains("url https://www.deezer.com/track/1"), "{calls}");
    assert_eq!(toml_value(&std::fs::read_to_string(config_path(&st)).unwrap(), "deezer", "arl"), "a-cookie", "the ARL from Settings is in the file before the run");
    // and a run that works
    std::fs::write(dir.join("fake/mode"), "ok").unwrap();
    let files = download(&st, "https://www.deezer.com/track/1", &out).await.unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("Album/01 Song.flac"), "{files:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- a Deezer album, end to end ---------------------------------------------------------------------

const DEEZER_ALBUM: &str = "https://www.deezer.com/album/302127";

/// Four small FLACs of "Illuminate EP" by Duumu, tagged the way streamrip tags them, in `dir`. False when
/// there is no ffmpeg here to make them.
pub(super) fn album_fixture(dir: &Path) -> bool {
    std::fs::create_dir_all(dir).unwrap();
    for n in 1..=4 {
        let out = dir.join(format!("{n:02}. Duumu - Light {n}.flac"));
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={}:duration=1", 220 * n)])
            .args(["-ar", "44100", "-sample_fmt", "s16", "-c:a", "flac"])
            .args(["-metadata", &format!("title=Light {n}"), "-metadata", "artist=Duumu", "-metadata", "album_artist=Duumu", "-metadata", "album=Illuminate EP"])
            .args(["-metadata", &format!("track={n}/4"), "-metadata", "date=2024"]).arg(&out).stdin(std::process::Stdio::null()).status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            return false;
        }
    }
    true
}

/// A cover for the request (served from here, so the import asks no catalogue for one).
pub(super) async fn cover_server(dir: &Path) -> Option<String> {
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

/// A server whose streamrip is the stand-in (Deezer signed in with an ARL), with its staging folder and
/// library in its own folder, nothing asked of MusicBrainz, no lyrics, no quality gate; and its folders.
struct Rip {
    ts: crate::test_support::TestServer,
    fake: PathBuf,
    staging: PathBuf,
    lib: PathBuf,
    cover: String,
}

async fn rip_server(name: &str) -> Option<Rip> {
    let ts = crate::test_support::TestServer::new(&format!("rip-e2e-{name}")).with_sources();
    let (fake, staging, lib) = (ts.dir.join("fake"), ts.dir.join("staging"), ts.dir.join("lib"));
    let rip = fake_rip(&fake);
    if !album_fixture(&fake.join("fixture")) {
        ts.remove();
        return None; // no ffmpeg here
    }
    let cover = cover_server(&ts.dir).await?;
    std::fs::create_dir_all(&lib).unwrap();
    ts.st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_deezer_arl": "arl-cookie",
        "staging_path": staging.to_string_lossy(), "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "autotag_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true,
        "source_order": "deezer", "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false }));
    crate::test_support::user(&ts.st, "boss", "admin"); // user 1, the requests' owner
    std::fs::write(fake.join("mode"), "album").unwrap();
    Some(Rip { ts, fake, staging, lib, cover })
}

/// The album picked in the picker: its Deezer release.
fn picked_album(st: &AppState, cover: &str) -> i64 {
    let t = encore_db::now();
    let detail = json!({ "pick": { "source": "deezer", "payload": { "url": DEEZER_ALBUM }, "title": "Illuminate EP", "subtitle": "Duumu" } });
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, cover_url, status, message, detail, created_at, updated_at) VALUES (1,'album','Duumu','Illuminate EP','Illuminate EP',?,'queued','',?,?,?)",
        p![cover, detail.to_string(), t, t]).unwrap()
}

pub(super) fn request_row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or_else(|_| json!({})))
}

pub(super) fn log_text(d: &Value) -> String {
    d["log"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Field `n` (1-based, after the command's parentheses) of a /proc/<pid>/stat line.
fn stat_field(stat: &str, n: usize) -> i64 {
    stat.rsplit_once(')').unwrap().1.split_whitespace().nth(n - 3).unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_picked_deezer_album_downloads_and_imports_through_the_worker() {
    let Some(r) = rip_server("ok").await else { return };
    let st = r.ts.st.clone();
    let id = picked_album(&st, &r.cover);
    let started = std::time::Instant::now();
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert!(message.starts_with("Downloaded from Deezer via streamrip"), "{message}");
    // four tracks in the library, indexed, from the request
    let n = st.db.one("SELECT COUNT(*) n FROM tracks WHERE request_id=?", p![id]).unwrap().unwrap().i("n");
    assert_eq!(n, 4);
    let files: Vec<PathBuf> = walkdir::WalkDir::new(&r.lib).into_iter().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "flac")).map(|e| e.into_path()).collect();
    assert_eq!(files.len(), 4, "{files:?}");
    assert!(files.iter().all(|f| f.starts_with(r.lib.join("Duumu"))), "{files:?}");
    assert!(!r.staging.join(format!("request-{id}")).exists(), "the staging folder goes after the import");
    // streamrip's output stays with the request when it worked too (both pipes)
    let output = d["output"]["text"].as_str().unwrap_or("");
    assert!(output.contains("Downloaded 4 tracks") && output.contains("WARNING: a warning on stderr"), "{d}");
    assert_eq!((d["output"]["ok"].as_bool(), d["output"]["url"].as_str()), (Some(true), Some(DEEZER_ALBUM)), "{d}");
    // the run: the default command line, no terminal (the /dev/tty write and read failed at once instead of
    // hanging), its own session, plain output, HOME the data folder
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
    let calls = std::fs::read_to_string(r.fake.join("calls.log")).unwrap();
    let run = calls.lines().find(|l| l.contains(" url ")).unwrap();
    let folder = r.staging.join(format!("request-{id}/streamrip"));
    assert!(run.contains(&format!("--folder {} --quality 2 --no-db --no-progress url {DEEZER_ALBUM}", folder.display())), "{run}");
    assert!(calls.lines().all(|l| l.contains("stdin=/dev/null")), "{calls}");
    let stat = std::fs::read_to_string(r.fake.join("stat")).unwrap();
    let mine = std::fs::read_to_string("/proc/self/stat").unwrap();
    assert_eq!(stat_field(&stat, 7), 0, "no controlling terminal: {stat}");
    assert_ne!(stat_field(&stat, 6), stat_field(&mine, 6), "a session of its own: {stat}");
    let env = std::fs::read_to_string(r.fake.join("env")).unwrap();
    assert_eq!(env.trim(), format!("TERM=dumb NO_COLOR=1 COLUMNS=200 PYTHONUNBUFFERED=1 HOME={}", st.data_dir().display()));
    // the config the run read: Deezer signed in, no downloads database, no progress bars, no update check
    let text = std::fs::read_to_string(config_path(&st)).unwrap();
    assert_eq!(toml_value(&text, "deezer", "arl"), "arl-cookie");
    assert_eq!(toml_value(&text, "database", "downloads_enabled"), "false");
    assert_eq!(toml_value(&text, "cli", "progress_bars"), "false");
    assert_eq!(toml_value(&text, "cli", "no_update_check"), "true", "{text}");
    r.ts.remove();
}

/// After an upgrade whose streamrip is still being installed (adopted, switched on, no program yet), a request
/// that needs it waits for it instead of going on to a lower-quality source or failing; once the program is
/// there it downloads from streamrip.
#[tokio::test]
async fn a_request_waits_for_a_streamrip_that_is_still_being_installed() {
    let Some(r) = rip_server("installing").await else { return };
    let st = r.ts.st.clone();
    let rip = st.cfg.str("streamrip_bin");
    // as the first start after an upgrade has it: the install pass not done, streamrip's program not there
    st.cfg.set_many(&json!({ "streamrip_bin": "" }));
    st.db.execute("INSERT OR REPLACE INTO plugin_state (key, value) VALUES ('migration', 'install')", p![]).unwrap();
    assert!(st.plugins.installing_after_upgrade("streamrip"));
    // a picked album and a song from the sources (Deezer first)
    let id = picked_album(&st, &r.cover);
    let t = encore_db::now();
    let song = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (1,'track','Duumu','Illuminate','queued','',?,?,?)",
        p![json!({ "album_mode": "song" }).to_string(), t, t]).unwrap();
    for _ in 0..2 {
        encore_requests::worker::step(st.clone(), id).await;
        encore_requests::worker::step(st.clone(), song).await;
    }
    let (status, message, d) = request_row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting for the streamrip plugin to finish installing"), "{}", log_text(&d));
    assert_eq!(d["wait"]["reason"], "plugin_install");
    assert_eq!(log_text(&d).matches("waiting for the streamrip plugin").count(), 1, "said once: {}", log_text(&d));
    let (status, message, d) = request_row(&st, song);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting for the streamrip plugin to finish installing"), "{}", log_text(&d));
    assert_eq!((d["sources"].clone(), d["install_wait"]["step"].clone()), (json!(["streamrip"]), json!("source")), "streamrip stays in its place: {d}");
    st.db.execute("UPDATE requests SET status='cancelled' WHERE id=?", p![song]).unwrap();
    // the program arrives (here: the stand-in): the picked album downloads from streamrip
    st.cfg.set_many(&json!({ "streamrip_bin": rip }));
    assert!(!st.plugins.installing_after_upgrade("streamrip") || encore_requests::sources::streamrip::binary(&st).is_some());
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert!(message.starts_with("Downloaded from Deezer via streamrip"), "{message}");
    assert!(d["wait"].is_null() && d["install_wait"].is_null(), "{d}");
    r.ts.remove();
}

#[tokio::test]
async fn a_run_hands_back_its_output_when_it_works_too() {
    let Some(r) = rip_server("output").await else { return };
    let st = r.ts.st.clone();
    let out = r.ts.dir.join("out");
    let got = download_run(&st, DEEZER_ALBUM, &out).await.unwrap();
    assert_eq!(got.files.len(), 4);
    for line in ["Downloading Illuminate EP by Duumu", "WARNING: a warning on stderr", "Downloaded 4 tracks"] {
        assert!(got.log.contains(line), "{line} in {}", got.log);
    }
    r.ts.remove();
}

#[tokio::test]
async fn a_staging_folder_that_cannot_be_made_names_mkdir_and_its_path() {
    let Some(r) = rip_server("mkdir").await else { return };
    let st = r.ts.st.clone();
    let id = picked_album(&st, &r.cover);
    let folder = r.staging.join(format!("request-{id}/streamrip"));
    // the staging folder itself is there and can be written (the download folders' probe passes; one that
    // can't be made holds the request up instead – server_tests/downloads.rs): only the request's own folder
    // in it can't be made
    std::fs::create_dir_all(&r.staging).unwrap();
    // what a read-only parent gives (as root, which may write anywhere, through the fault seam)
    fsx::fault::fail(&r.staging, "mkdir", 13);
    encore_requests::worker::step(st.clone(), id).await;
    fsx::fault::clear(&r.staging);
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "failed", "{message}");
    assert!(message.contains(&format!("mkdir {}: Permission denied (os error 13)", folder.display())), "{message}");
    let calls = std::fs::read_to_string(r.fake.join("calls.log")).unwrap_or_default();
    assert!(!calls.contains(" url "), "streamrip never ran: {calls}");
    assert!(log_text(&d).contains(&format!("mkdir {}", folder.display())), "{d}");
    r.ts.remove();
}

#[tokio::test]
async fn a_config_that_cannot_be_written_names_write_and_the_config() {
    let Some(r) = rip_server("config").await else { return };
    let st = r.ts.st.clone();
    let id = picked_album(&st, &r.cover);
    let config = config_path(&st);
    fsx::fault::fail(config.parent().unwrap(), "write", 5);
    encore_requests::worker::step(st.clone(), id).await;
    fsx::fault::clear(config.parent().unwrap());
    let (status, message, _) = request_row(&st, id);
    assert_eq!(status, "failed", "{message}");
    assert!(message.contains(&format!("config failed: write {}: Input/output error (os error 5)", config.display())), "{message}");
    let calls = std::fs::read_to_string(r.fake.join("calls.log")).unwrap_or_default();
    assert!(!calls.contains(" url "), "streamrip never ran: {calls}");
    r.ts.remove();
}

#[tokio::test]
async fn a_failed_run_keeps_streamrips_words_and_its_whole_output() {
    let Some(r) = rip_server("fail").await else { return };
    let st = r.ts.st.clone();
    std::fs::write(r.fake.join("mode"), "fail").unwrap();
    let id = picked_album(&st, &r.cover);
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "failed", "{message}");
    // the message says it in plain words (2318cac); streamrip's own words are in the output below
    assert!(message.contains(encore_requests::sources::streamrip::NOT_STREAMABLE_MSG), "{message}");
    // the whole output, both pipes, in the request's details (once: the log says where it is)
    let output = d["output"]["text"].as_str().unwrap_or("").to_string();
    // (the stand-in's write to /dev/tty is not in it: there is no terminal, and the shell said so)
    assert!(!output.contains("Logging in to Deezer") && output.contains("/dev/tty: No such device or address"), "{output}");
    for line in ["first line of the run, kept in the log", "Traceback (most recent call last):", "File \"rip/main.py\", line 120, in rip",
                 "NonStreamableError: This album is not available in your country"] {
        assert!(output.contains(line), "{line} in {output}");
    }
    assert_eq!((d["output"]["ok"].as_bool(), d["output"]["url"].as_str()), (Some(false), Some(DEEZER_ALBUM)), "{d}");
    let log = log_text(&d);
    assert!(log.contains(&format!("streamrip: {DEEZER_ALBUM} failed – its output is in the details")), "{log}");
    assert!(!log.contains("first line of the run"), "the output is not in the log a second time: {log}");
    assert_eq!(walkdir::WalkDir::new(&r.lib).into_iter().flatten().filter(|e| e.file_type().is_file()).count(), 0);
    r.ts.remove();
}

/// EIO on the move into the library: the request waits for the storage with its files kept, no other source
/// is tried and no attempt is counted; once the disk is back (Check again) the import alone runs and
/// completes it.
#[tokio::test]
async fn eio_on_the_move_into_the_library_waits_with_the_files_kept_then_imports() {
    let Some(r) = rip_server("eio").await else { return };
    let st = r.ts.st.clone();
    let id = picked_album(&st, &r.cover);
    fsx::fault::fail(&r.lib, "*", 5);
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert!(message.starts_with("Waiting: library storage error – import failed: "), "{message}");
    assert!(message.contains("Input/output error (os error 5)"), "{message}");
    assert_eq!(d["attempts"].as_i64().unwrap_or(0), 0, "{d}");
    let kept = walkdir::WalkDir::new(r.staging.join(format!("request-{id}"))).into_iter().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "flac")).count();
    assert_eq!(kept, 4, "the downloaded files stay in staging");
    let runs = || std::fs::read_to_string(r.fake.join("calls.log")).unwrap_or_default().lines().filter(|l| l.contains(" url ")).count();
    assert_eq!(runs(), 1);
    // still failing: the queue's look leaves it waiting, nothing downloaded again
    encore_requests::worker::step(st.clone(), id).await;
    assert_eq!(request_row(&st, id).0, "waiting");
    assert_eq!(runs(), 1);
    // the disk is back and an admin presses Check again: the import runs again from the kept files
    fsx::fault::clear(&r.lib);
    let health = encore_requests::worker::storage::check_now(&st, true).await;
    assert_eq!(health["ok"], true, "{health}");
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(runs(), 1, "never downloaded again");
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM tracks WHERE request_id=?", p![id]).unwrap().unwrap().i("n"), 4);
    r.ts.remove();
}

/// streamrip's own disk error while it writes into the staging folder ("[Errno 5] Input/output error:
/// '…/request-1/streamrip/…'") is the storage's problem, not Deezer's: the download step waits (no attempt
/// counted, nothing else tried) and runs again once its turn comes and the disk is fine.
#[tokio::test]
async fn streamrips_own_disk_error_in_staging_waits_for_the_storage() {
    let Some(r) = rip_server("errno").await else { return };
    let st = r.ts.st.clone();
    std::fs::write(r.fake.join("mode"), "errno").unwrap();
    let id = picked_album(&st, &r.cover);
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!((status.as_str(), d["wait"]["reason"].as_str()), ("waiting", Some("storage_download")), "{message}");
    let file = r.staging.join(format!("request-{id}/streamrip/Duumu - Illuminate EP/01. Light 1.flac"));
    assert!(message.starts_with(&format!("Waiting: storage error – download failed: write {}: Input/output error (os error 5). Next try at ", file.display())), "{message}");
    assert_eq!(d["attempts"].as_i64().unwrap_or(0), 0, "{d}");
    assert!(d["output"]["text"].as_str().unwrap_or("").contains("[Errno 5] Input/output error"), "{d}");
    let runs = || std::fs::read_to_string(r.fake.join("calls.log")).unwrap_or_default().lines().filter(|l| l.contains(" url ")).count();
    assert_eq!(runs(), 1);
    // its turn, the disk fine again: the download runs again and the album is imported
    std::fs::write(r.fake.join("mode"), "album").unwrap();
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', ?) WHERE id=?", p![encore_db::now() - 1.0, id]).unwrap();
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(runs(), 2);
    assert_eq!(d["attempts"].as_i64().unwrap_or(0), 0, "{d}");
    r.ts.remove();
}

/// The real streamrip against Deezer, when asked to: `ENCORE_LIVE_DEEZER_URL` (an album or track link) and
/// `ENCORE_LIVE_DEEZER_ARL` set, and the streamrip on PATH (or `ENCORE_LIVE_STREAMRIP_BIN`).
/// `cargo test -p encore-api deezer_album_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "runs the real streamrip against Deezer: set ENCORE_LIVE_DEEZER_URL and ENCORE_LIVE_DEEZER_ARL"]
async fn deezer_album_live() {
    let (Ok(url), Ok(arl)) = (std::env::var("ENCORE_LIVE_DEEZER_URL"), std::env::var("ENCORE_LIVE_DEEZER_ARL")) else {
        eprintln!("ENCORE_LIVE_DEEZER_URL and ENCORE_LIVE_DEEZER_ARL are not set: nothing to do");
        return;
    };
    let (st, dir) = state("live");
    let mut settings = json!({ "streamrip_enabled": true, "streamrip_deezer_arl": arl, "streamrip_quality": 2 });
    if let Ok(bin) = std::env::var("ENCORE_LIVE_STREAMRIP_BIN") {
        settings["streamrip_bin"] = json!(bin);
    }
    st.cfg().set_many(&settings);
    let bin = binary(&st).expect("streamrip installed (PATH, /opt/tools/bin or ENCORE_LIVE_STREAMRIP_BIN)");
    let c = check_config(&st, false).await.unwrap();
    assert!(c.problem.is_none(), "{}", c.message());
    let out = dir.join("out");
    let started = std::time::Instant::now();
    let res = download_run(&st, &url, &out).await;
    match &res {
        Ok(r) => eprintln!("{bin}: {} file(s) in {:?}\n{}", r.files.len(), started.elapsed(), r.log),
        Err(e) => eprintln!("{bin}: {e:#}\n{}", run_log(e).unwrap_or("")),
    }
    let got = res.unwrap();
    assert!(!got.files.is_empty());
    let text = std::fs::read_to_string(config_path(&st)).unwrap();
    assert_eq!(toml_value(&text, "cli", "progress_bars"), "false");
    let _ = std::fs::remove_dir_all(&dir);
}
