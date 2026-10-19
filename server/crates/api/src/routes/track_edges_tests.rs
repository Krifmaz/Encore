//! Song edges: silencedetect's log parsed into where the sound starts and stops, and the endpoint on real
//! generated audio (silence, a tone, silence) – measured once, then served from the database.
use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[test]
fn leading_and_trailing_silence_are_trimmed() {
    let log = "[silencedetect @ 0x1] silence_start: 0\n[silencedetect @ 0x1] silence_end: 1.502 | silence_duration: 1.502\n\
               size=N/A time=00:00:06.50\n[silencedetect @ 0x1] silence_start: 4.498\n[silencedetect @ 0x1] silence_end: 6.5 | silence_duration: 2.002\n";
    assert_eq!(parse_silence(log, 6.5), (1.502, 4.498));
}

#[test]
fn trailing_silence_without_an_end_line_counts_to_the_end() {
    // older ffmpeg: no silence_end when the silence runs into the end of the file
    let log = "silence_start: 200.25\n";
    assert_eq!(parse_silence(log, 210.0), (0.0, 200.25));
}

#[test]
fn trailing_silence_ending_where_ffmpeg_stopped_decoding_counts() {
    // the tagged length (lofty) is longer than what ffmpeg decodes: its EOF silence_end is at the decoded end
    let log = "silence_start: 200.25\nsilence_end: 209.6 | silence_duration: 9.35\nsize=N/A time=00:03:29.60 bitrate=N/A speed=500x\n";
    assert_eq!(parse_silence(log, 210.0), (0.0, 200.25));
    // a quiet passage that ends before the decoded end stays inside the song
    let log = "silence_start: 100\nsilence_end: 102 | silence_duration: 2\nsize=N/A time=00:03:29.60 bitrate=N/A\n";
    assert_eq!(parse_silence(log, 210.0), (0.0, 210.0));
}

#[test]
fn quiet_passages_inside_the_song_are_not_edges() {
    let log = "silence_start: 50\nsilence_end: 52 | silence_duration: 2\n";
    assert_eq!(parse_silence(log, 180.0), (0.0, 180.0));
    assert_eq!(parse_silence("", 180.0), (0.0, 180.0));
}

#[test]
fn a_silent_file_keeps_its_whole_length() {
    let log = "silence_start: 0\nsilence_end: 30 | silence_duration: 30\n";
    assert_eq!(parse_silence(log, 30.0), (0.0, 30.0));
}

#[test]
fn duration_is_read_from_the_input_description() {
    assert_eq!(parse_duration("  Duration: 00:03:21.50, start: 0.000000, bitrate: 900 kb/s"), Some(201.5));
    assert_eq!(parse_duration("nothing"), None);
}

#[tokio::test]
async fn endpoint_measures_generated_audio_once_and_caches_it() {
    let dir = std::env::temp_dir().join(format!("encore-edges-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 1.5 s of silence, 3 s of a tone, 2 s of silence
    let f = dir.join("edges.flac");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo:d=1.5", "-f", "lavfi", "-i", "sine=frequency=440:duration=3:sample_rate=44100",
               "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo:d=2",
               "-filter_complex", "[1]aformat=channel_layouts=stereo[t];[0][t][2]concat=n=3:v=0:a=1"])
        .arg(&f).status();
    if !ok.map(|s| s.success()).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&dir);
        return; // no ffmpeg here
    }
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "edger", "edges-pass-1", "user", None, None).unwrap();
    let size = std::fs::metadata(&f).unwrap().len() as i64;
    st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, format, duration, mtime, size) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![7, f.to_string_lossy(), "Edges", "Tester", "A", "a1", "flac", 6.5, 1000.0, size]).unwrap();
    let app = crate::app(st.clone());
    let login = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "edger", "password": "edges-pass-1" }).to_string())).unwrap();
    let resp = app.clone().oneshot(login).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let get = |uri: &str| Request::builder().uri(uri).header("cookie", &cookie).header("x-requested-with", "encore").body(Body::empty()).unwrap();

    let read = |resp: axum::response::Response| async move {
        let b = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice::<Value>(&b).unwrap()
    };
    let resp = app.clone().oneshot(get("/api/library/tracks/7/edges")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = read(resp).await;
    let (s, e) = (v["start_s"].as_f64().unwrap(), v["end_s"].as_f64().unwrap());
    assert!((s - 1.5).abs() < 0.1, "start {s}");
    assert!((e - 4.5).abs() < 0.1, "end {e}");
    assert_eq!(v["cached"], json!(false));
    // the second time from the database
    let v = read(app.clone().oneshot(get("/api/library/tracks/7/edges")).await.unwrap()).await;
    assert_eq!(v["cached"], json!(true));
    assert_eq!((v["start_s"].as_f64().unwrap(), v["end_s"].as_f64().unwrap()), (s, e));
    // the file changed: measured again
    st.db.execute("UPDATE tracks SET mtime=2000 WHERE id=7", p![]).unwrap();
    let v = read(app.clone().oneshot(get("/api/library/tracks/7/edges")).await.unwrap()).await;
    assert_eq!(v["cached"], json!(false));
    // unknown song
    let resp = app.clone().oneshot(get("/api/library/tracks/99/edges")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(&dir);
}
