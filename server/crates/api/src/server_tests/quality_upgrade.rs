//! Quality upgrades on a whole server (encore_requests::upgrade): the album page rates each album by its
//! worst song (`quality_summary`), ⋯ → Upgrade quality asks for the next better copy – lossless for a lossy
//! album, hi-res for a CD-quality one, which needs streamrip with Qobuz or Tidal – and refuses what it can't
//! improve, and the Hi-res upgrades task queues CD-quality albums only.
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::Path;

/// Logs `name` in and answers its session cookie.
async fn login(app: &axum::Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// A two-second tagged tone at `path`: FLAC at `bits` / `rate` Hz, or a 320 kbps MP3 for an .mp3 path.
fn tone(path: &Path, album: &str, bits: u8, rate: u32) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut c = std::process::Command::new("ffmpeg");
    c.args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency=330:duration=2:sample_rate={rate}"), "-ac", "2"]);
    if path.extension().is_some_and(|e| e == "mp3") {
        c.args(["-c:a", "libmp3lame", "-b:a", "320k"]);
    } else {
        // 24-bit FLAC is stored from 32-bit samples
        c.args(["-c:a", "flac", "-sample_fmt", if bits > 16 { "s32" } else { "s16" }]);
        if bits > 16 {
            c.args(["-bits_per_raw_sample", "24"]);
        }
    }
    for (k, v) in [("title", "Sine"), ("artist", "Tone"), ("album_artist", "Tone"), ("album", album), ("track", "1")] {
        c.args(["-metadata", &format!("{k}={v}")]);
    }
    c.arg(path).status().is_ok_and(|s| s.success())
}

/// The album id of the album named `album`.
fn album_id(st: &crate::state::AppState, album: &str) -> String {
    st.db.one("SELECT album_id FROM tracks WHERE album=?", p![album]).unwrap().unwrap().s("album_id")
}

#[tokio::test]
async fn albums_are_rated_and_upgraded_one_tier_at_a_time() {
    if encore_core::which::which("ffmpeg").is_none() {
        return;
    }
    let ts = TestServer::new("quality-upgrade");
    let lib = ts.dir.join("music");
    let ts = ts.with(json!({ "library_path": lib.to_string_lossy() }));
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    user(&st, "member", "user");
    assert!(tone(&lib.join("Tone/Rip/01 Sine.mp3"), "Rip", 0, 44_100));
    assert!(tone(&lib.join("Tone/Disc/01 Sine.flac"), "Disc", 16, 44_100));
    assert!(tone(&lib.join("Tone/Master/01 Sine.flac"), "Master", 24, 96_000));
    st.library.scan_library(st.db.clone(), &st.cfg, false).await;
    // each album matched to its own MusicBrainz release group
    for a in ["Rip", "Disc", "Master"] {
        st.db.execute("UPDATE tracks SET mb_releasegroupid=? WHERE album=?", p![format!("rg-{a}"), a]).unwrap();
    }
    // a hi-res upgrade's download check: the 24/96 file is hi-res, the CD one and the MP3 aren't
    use encore_requests::upgrade::files_are_hires;
    assert!(files_are_hires(&[lib.join("Tone/Disc/01 Sine.flac"), lib.join("Tone/Master/01 Sine.flac")]));
    assert!(!files_are_hires(&[lib.join("Tone/Disc/01 Sine.flac"), lib.join("Tone/Rip/01 Sine.mp3")]));
    let (rip, disc, master) = (album_id(&st, "Rip"), album_id(&st, "Disc"), album_id(&st, "Master"));
    let app = ts.app();
    let boss = login(&app, "boss").await;
    let member = login(&app, "member").await;

    // the album page says how good each one is and what an upgrade would ask for
    let summary = |v: &Value| (v["quality_summary"]["tier"].as_str().map(String::from), v["quality_summary"]["upgrade"].as_str().map(String::from));
    let (_, a, _) = call(&app, "GET", &format!("/api/library/albums/{rip}"), &member, None).await;
    assert_eq!(summary(&a), (Some("lossy".into()), Some("lossless".into())));
    // (a two-second file's bitrate reads a little above what it was encoded at)
    assert!(a["quality_summary"]["line"].as_str().is_some_and(|l| l.starts_with("MP3 · 3") && l.ends_with(" kbps")), "{}", a["quality_summary"]);
    let (_, a, _) = call(&app, "GET", &format!("/api/library/albums/{disc}"), &member, None).await;
    assert_eq!(summary(&a), (Some("lossless".into()), Some("hires".into())));
    assert_eq!(a["quality_summary"]["line"], "FLAC · 16-bit / 44.1 kHz");
    let (_, a, _) = call(&app, "GET", &format!("/api/library/albums/{master}"), &member, None).await;
    assert_eq!(summary(&a), (Some("hires".into()), None));
    assert_eq!(a["quality_summary"]["line"], "FLAC · 24-bit / 96 kHz");

    // only admins upgrade
    let (code, _, _) = call(&app, "POST", &format!("/api/library/albums/{rip}/upgrade"), &member, None).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    // a hi-res album has nothing better; a CD-quality one needs streamrip with Qobuz or Tidal
    let (code, _, _) = call(&app, "POST", &format!("/api/library/albums/{master}/upgrade"), &boss, None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    let (code, v, _) = call(&app, "POST", &format!("/api/library/albums/{disc}/upgrade"), &boss, None).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(encore_requests::upgrade::queue_hires_upgrades(&st, 10).unwrap(), 0, "no hi-res source: nothing queued");
    assert_eq!(encore_requests::upgrade::cd_quality_albums(&st).unwrap(), 1);

    // the lossy album: a lossless upgrade, once
    let (code, v, _) = call(&app, "POST", &format!("/api/library/albums/{rip}/upgrade"), &boss, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["tier"], "lossless");
    let r = st.db.one("SELECT * FROM requests WHERE id=?", p![v["request_id"]]).unwrap().unwrap();
    let d: Value = serde_json::from_str(&r.s("detail")).unwrap();
    assert_eq!((r.s("kind").as_str(), r.s("mbid").as_str(), d["lossless_only"].as_bool(), d["hires_only"].as_bool()), ("album", "rg-Rip", Some(true), None));
    let (code, _, _) = call(&app, "POST", &format!("/api/library/albums/{rip}/upgrade"), &boss, None).await;
    assert_eq!(code, StatusCode::CONFLICT, "an upgrade under way is not asked for twice");

    // streamrip on Qobuz: the CD-quality album gets a hi-res upgrade, by hand and not again by the task
    st.plugins.adopt("streamrip", None).unwrap();
    st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": "/bin/true", "source_order": "qobuz,deezer,slskd" }));
    assert!(encore_requests::upgrade::hires_source(&st));
    let (code, v, _) = call(&app, "POST", &format!("/api/library/albums/{disc}/upgrade"), &boss, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["tier"], "hires");
    let r = st.db.one("SELECT detail, message FROM requests WHERE id=?", p![v["request_id"]]).unwrap().unwrap();
    let d: Value = serde_json::from_str(&r.s("detail")).unwrap();
    assert_eq!((d["lossless_only"].as_bool(), d["hires_only"].as_bool(), r.s("message").as_str()), (Some(true), Some(true), "Hi-res upgrade"));
    assert_eq!(encore_requests::upgrade::queue_hires_upgrades(&st, 10).unwrap(), 0, "asked for just now");

    // the task on its own: the CD-quality album once its last ask is older than the retry interval
    st.db.execute("DELETE FROM requests", p![]).unwrap();
    st.db.execute("DELETE FROM sync_state WHERE scope LIKE 'hires:%'", p![]).unwrap();
    assert_eq!(encore_requests::upgrade::queue_hires_upgrades(&st, 10).unwrap(), 1);
    assert_eq!(st.db.one("SELECT mbid FROM requests", p![]).unwrap().unwrap().s("mbid"), "rg-Disc");
    ts.remove();
}
