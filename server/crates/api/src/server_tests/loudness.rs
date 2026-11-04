//! Volume normalization's data end to end (0.31): the scan reads a song's ReplayGain tags and its sample
//! rate and bit depth, the "Measure loudness" task measures the other one with the real ffmpeg, and the
//! answers the apps read carry it: the track rows (`gain`, `sample_rate`, `bit_depth`), the album page and
//! the signal path (`GET /api/library/tracks/{id}/info`).
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::Path;

/// A sine as FLAC (ffmpeg's default level, 1/8 of full scale) with these Vorbis comments; false when ffmpeg
/// isn't installed.
fn flac(path: &Path, rate: u32, secs: u32, tags: &[(&str, &str)]) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", &format!("sine=f=440:d={secs}:sample_rate={rate}"), "-c:a", "flac"]);
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    cmd.arg(path).status().is_ok_and(|s| s.success())
}

fn by_id(rows: &Value, id: i64) -> Value {
    rows.as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn songs_carry_their_loudness_from_tags_and_measurement() {
    let s = TestServer::new("loudness");
    let music = s.dir.join("music");
    let (tagged, plain) = (music.join("Ann/Alb/01.flac"), music.join("Ann/Alb/02.flac"));
    let rg = [("title", "Tagged"), ("artist", "Ann"), ("album", "Alb"), ("REPLAYGAIN_TRACK_GAIN", "-7.21 dB"), ("REPLAYGAIN_TRACK_PEAK", "0.988"),
              ("REPLAYGAIN_ALBUM_GAIN", "-6.84 dB"), ("REPLAYGAIN_ALBUM_PEAK", "1.0")];
    if !flac(&tagged, 96000, 1, &rg) || !flac(&plain, 44100, 3, &[("title", "Plain"), ("artist", "Ann"), ("album", "Alb")]) {
        eprintln!("ffmpeg is not installed: skipped");
        s.remove();
        return;
    }
    let st = s.st.clone();
    let lib = music.to_string_lossy().to_string();
    st.cfg.set_many(&json!({ "library_path": lib }));
    st.library.scan(st.db.clone(), lib.clone(), false).await;
    let id_of = |p: &Path| st.db.one("SELECT id FROM tracks WHERE path=?", p![p.to_string_lossy()]).unwrap().unwrap().i("id");
    let (a, b) = (id_of(&tagged), id_of(&plain));
    let row = st.db.one("SELECT loudness_source, loudness_lufs, sample_rate, bit_depth FROM tracks WHERE id=?", p![a]).unwrap().unwrap();
    assert_eq!((row.s("loudness_source"), row.f("loudness_lufs"), row.i("sample_rate"), row.i("bit_depth")), ("tags".to_string(), -10.79, 96000, 16));
    let row = st.db.one("SELECT loudness_source, sample_rate FROM tracks WHERE id=?", p![b]).unwrap().unwrap();
    assert_eq!((row.s("loudness_source"), row.i("sample_rate")), (String::new(), 44100), "not measured yet");

    // the task measures the plain song (before any call, so the busy gate lets it go at once)
    let line = encore_library::loudness::scheduled(&st).await.unwrap();
    assert_eq!(line, "1 measured");
    let row = st.db.one("SELECT loudness_source, loudness_lufs, rg_track_gain, rg_track_peak FROM tracks WHERE id=?", p![b]).unwrap().unwrap();
    assert_eq!(row.s("loudness_source"), "measured");
    // a sine at 1/8 of full scale: about -21.8 LUFS, a peak of about 0.125
    let lufs = row.f("loudness_lufs");
    assert!((-24.0..-19.0).contains(&lufs), "{lufs}");
    assert!((row.f("rg_track_gain") - (-18.0 - lufs)).abs() < 0.011, "{row:?}");
    assert!((0.1..0.16).contains(&row.f("rg_track_peak")), "{row:?}");
    // the next run has nothing to do
    assert_eq!(encore_library::loudness::scheduled(&st).await.unwrap(), "Nothing left to measure");

    let app = s.app();
    user(&st, "lou", "user");
    let (code, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "lou", "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    let cookie = cookie.split(';').next().unwrap().to_string();
    // the album gain: one song has the tag, the other not: the energy mean of both songs' loudness
    let album_gain = -18.0 - 10.0 * ((10f64.powf(-10.79 / 10.0) + 10f64.powf(lufs / 10.0)) / 2.0).log10();
    let (code, rows, _) = call(&app, "GET", &format!("/api/library/tracks?ids={a},{b}"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    let ta = by_id(&rows, a);
    assert_eq!(ta["gain"]["track"], -7.21);
    assert_eq!(ta["gain"]["track_peak"], 0.988);
    assert!((ta["gain"]["album"].as_f64().unwrap() - album_gain).abs() < 0.011, "{ta}");
    assert_eq!(ta["gain"]["album_peak"], 0.988, "the loudest track peak: not every song has the album peak tag");
    assert_eq!((ta["sample_rate"].clone(), ta["bit_depth"].clone(), ta["format"].clone()), (json!(96000), json!(16), json!("flac")));
    let tb = by_id(&rows, b);
    assert_eq!(tb["gain"]["album"], ta["gain"]["album"]);
    assert_eq!(tb["sample_rate"], 44100);
    // the album page: the same, and none of the raw columns
    let album_id = st.db.one("SELECT album_id FROM tracks WHERE id=?", p![a]).unwrap().unwrap().s("album_id");
    let (code, alb, _) = call(&app, "GET", &format!("/api/library/albums/{album_id}"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    let t = by_id(&alb["tracks"], a);
    assert_eq!(t["gain"], ta["gain"]);
    for raw in ["rg_track_gain", "rg_track_peak", "rg_album_gain", "rg_album_peak", "loudness_lufs", "loudness_source"] {
        assert!(t.get(raw).is_none(), "{raw} in {t}");
    }
    // the signal path: where it came from, and the loudness
    let (code, info, _) = call(&app, "GET", &format!("/api/library/tracks/{b}/info"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(info["gain"]["source"], "measured");
    assert!((info["gain"]["lufs"].as_f64().unwrap() - lufs).abs() < 0.011, "{info}");
    assert_eq!(info["gain"]["track"], tb["gain"]["track"]);
    let (_, info, _) = call(&app, "GET", &format!("/api/library/tracks/{a}/info"), &cookie, None).await;
    assert_eq!((info["gain"]["source"].clone(), info["gain"]["lufs"].clone()), (json!("tags"), json!(-10.79)));
    // one song, and the shuffle the web player continues with (POST)
    let (_, one, _) = call(&app, "GET", &format!("/api/library/tracks/{a}"), &cookie, None).await;
    assert_eq!(one["gain"], ta["gain"]);
    let (code, shuffled, _) = call(&app, "POST", "/api/library/shuffle", &cookie, Some(json!({ "limit": 5 }))).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(by_id(&shuffled, a)["gain"], ta["gain"]);
    s.remove();
}
