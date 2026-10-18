//! Codec negotiation through the real router: a client that lists its decoders (`X-Encore-Decoders` or
//! `?codecs=`) gets the original when the file's codec is listed and a lossless FLAC fallback otherwise, the
//! `x-encore-stream` label says which, `/tracks/{id}/info` makes the same call, DSF passes through when
//! listed, and `audio_info` is read once per file and mtime. `offline_ok` in `/info` follows the plan, and only
//! the original may be kept by a cache: every stream made on the fly says `private, no-store`.
use crate::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

const PW: &str = "stream-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    music: PathBuf,
    cookie: String,
    next: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-stream-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    crate::auth::create_user(&st, "streamer", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "streamer", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let music = dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    Env { st, app, dir, music, cookie, next: 1 }
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn h(&self, name: &str) -> String {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    /// (sample rate, channels, bits per sample) from the STREAMINFO block of a FLAC body.
    fn flac_info(&self) -> (u32, u32, u32) {
        let b = &self.body;
        assert_eq!(&b[..4], b"fLaC", "not FLAC: {:?}", &b[..4.min(b.len())]);
        assert_eq!(b[4] & 0x7f, 0, "the first metadata block is STREAMINFO");
        let rate = (b[18] as u32) << 12 | (b[19] as u32) << 4 | (b[20] as u32) >> 4;
        let channels = ((b[20] as u32) >> 1 & 7) + 1;
        let bits = ((b[20] as u32 & 1) << 4 | (b[21] as u32) >> 4) + 1;
        (rate, channels, bits)
    }
}

impl Env {
    /// A song row for a file in the music folder (`format` is the extension, as the scan stores it).
    fn add(&mut self, file: &str) -> i64 {
        let id = self.next;
        self.next += 1;
        let path = self.music.join(file);
        let fmt = path.extension().unwrap().to_string_lossy().to_lowercase();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as i64;
        self.st.db.execute(
            "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, path.to_string_lossy(), file, "Tester", "Tones", "Tester", "tester|tones", fmt, 0, 1.0, size, "2024", 1, id, id as f64]).unwrap();
        id
    }
    async fn get(&self, uri: &str, decoders: Option<&str>) -> Resp {
        let mut b = Request::builder().method("GET").uri(uri).header("x-requested-with", "encore").header("cookie", &self.cookie);
        if let Some(d) = decoders {
            b = b.header("X-Encore-Decoders", d);
        }
        let resp = self.app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
        Resp { status, headers, body }
    }
    async fn stream(&self, id: i64, query: &str, decoders: Option<&str>) -> Resp {
        let r = self.get(&format!("/api/library/tracks/{id}/stream{query}"), decoders).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        r
    }
    async fn info(&self, id: i64, query: &str, decoders: Option<&str>) -> Value {
        let r = self.get(&format!("/api/library/tracks/{id}/info{query}"), decoders).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        r.json()["stream"].clone()
    }
}

fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// One second of a sine tone as `codec` (`libopus`, `flac` …) at `rate` Hz, stereo.
fn make(path: &std::path::Path, rate: u32, codec: &str) -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", &format!("sine=f=440:d=1:sample_rate={rate}"), "-ac", "2", "-c:a", codec])
        .arg(path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// A small DSF file: the `DSD `, `fmt ` and `data` chunks, stereo DSD64, four 4 KB blocks per channel of
/// the 0x69 pattern (what ffmpeg's dsf demuxer and a DSD DAC both read as near silence).
fn dsf_bytes() -> Vec<u8> {
    let (channels, block, blocks) = (2u32, 4096u32, 4u32);
    let per_channel = block * blocks;
    let data = vec![0x69u8; (per_channel * channels) as usize];
    let total = 28 + 52 + 12 + data.len() as u64;
    let mut v = Vec::new();
    v.extend_from_slice(b"DSD ");
    v.extend_from_slice(&28u64.to_le_bytes());
    v.extend_from_slice(&total.to_le_bytes());
    v.extend_from_slice(&0u64.to_le_bytes()); // no metadata chunk
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&52u64.to_le_bytes());
    for n in [1u32, 0, 2, channels, 2_822_400, 1] { // version, format id (raw DSD), channel type (stereo), channels, rate, bits per sample
        v.extend_from_slice(&n.to_le_bytes());
    }
    v.extend_from_slice(&((per_channel * 8) as u64).to_le_bytes()); // samples per channel
    v.extend_from_slice(&block.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(12 + data.len() as u64).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// A WAV file (16-bit stereo silence, a few ms) at `rate` Hz: enough for lofty to read its properties.
fn wav_bytes(rate: u32) -> Vec<u8> {
    let data = vec![0u8; (rate / 100 * 4) as usize];
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&2u16.to_le_bytes()); // channels
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes per second
    v.extend_from_slice(&4u16.to_le_bytes()); // block align
    v.extend_from_slice(&16u16.to_le_bytes()); // bits
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

#[tokio::test]
async fn the_original_when_the_codec_is_listed_and_flac_otherwise() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not found: skipping the codec negotiation check");
        return;
    }
    let mut e = setup("negotiate").await;
    if !make(&e.music.join("a.opus"), 48_000, "libopus") {
        eprintln!("this ffmpeg can't make Opus: skipping the codec negotiation check");
        return;
    }
    assert!(make(&e.music.join("b.flac"), 44_100, "flac"));
    let opus = e.add("a.opus");
    let flac = e.add("b.flac");

    // no list: what a browser plays comes as is (today's behaviour)
    let r = e.stream(opus, "", None).await;
    assert_eq!((r.h("x-encore-stream").as_str(), r.h("content-type").as_str()), ("original", "audio/ogg"));
    assert_eq!(&r.body[..4], b"OggS");
    assert_eq!(r.h("accept-ranges"), "bytes", "the original is seekable");
    assert!(r.h("x-transcoded").is_empty());

    // the list has opus: the original
    let r = e.stream(opus, "", Some("flac,mp3,aac,alac,opus,vorbis,wav,aiff,dsf,dff")).await;
    assert_eq!(r.h("x-encore-stream"), "original");
    assert_eq!(&r.body[..4], b"OggS");
    assert_eq!(r.h("vary"), "X-Encore-Decoders", "a cache keeps originals apart per decoder list");

    // the list lacks opus (an engine without an Opus decoder): lossless FLAC at the file's 48 kHz, streamed, no Range
    let r = e.stream(opus, "", Some("flac, MP3, aac")).await;
    assert_eq!((r.h("x-encore-stream").as_str(), r.h("x-transcoded").as_str(), r.h("content-type").as_str()), ("flac-fallback", "flac-fallback", "audio/flac"));
    let (rate, channels, bits) = r.flac_info();
    assert_eq!((rate, channels), (48_000, 2));
    assert!(bits == 16 || bits == 24, "{bits}");
    assert!(r.h("accept-ranges").is_empty() && r.h("content-length").is_empty(), "a made stream has no Range or length");

    // the same list on the URL
    let r = e.stream(opus, "?codecs=flac,mp3", None).await;
    assert_eq!(r.h("x-encore-stream"), "flac-fallback");
    assert_eq!(&r.body[..4], b"fLaC");
    let r = e.stream(opus, "?codecs=opus", None).await;
    assert_eq!(r.h("x-encore-stream"), "original");
    // both given: the header wins; an empty list is no list
    let r = e.stream(opus, "?codecs=mp3", Some("opus")).await;
    assert_eq!(r.h("x-encore-stream"), "original");
    let r = e.stream(opus, "?codecs=", Some("")).await;
    assert_eq!(r.h("x-encore-stream"), "original");

    // a FLAC file is the original whatever the list says: the fallback would be FLAC too
    let r = e.stream(flac, "", Some("mp3")).await;
    assert_eq!((r.h("x-encore-stream").as_str(), r.h("content-type").as_str()), ("original", "audio/flac"));
    assert_eq!(r.flac_info(), (44_100, 2, 16));

    // the client's own switches win over the list: an asked-for conversion is `flac`, not the fallback
    let r = e.stream(opus, "?transcode=1", Some("opus")).await;
    assert_eq!(r.h("x-encore-stream"), "flac");
    assert_eq!(&r.body[..4], b"fLaC");

    // /info makes the same call, and says why
    assert_eq!(e.info(opus, "", None).await, json!({ "kind": "original", "codec": "opus", "reason": null }));
    assert_eq!(e.info(opus, "", Some("flac,opus")).await, json!({ "kind": "original", "codec": "opus", "reason": null }));
    let s = e.info(opus, "", Some("flac,mp3,aac")).await;
    assert_eq!((s["kind"].as_str(), s["codec"].as_str()), (Some("flac-fallback"), Some("opus")));
    assert_eq!(s["reason"], "the client has no Opus decoder");
    assert_eq!(e.info(opus, "?codecs=flac", None).await["kind"], "flac-fallback");
    assert_eq!(e.info(flac, "", Some("mp3")).await, json!({ "kind": "original", "codec": "flac", "reason": null }));
    let full = e.get(&format!("/api/library/tracks/{opus}/info"), None).await.json();
    assert_eq!((full["codec"].as_str(), full["sample_rate"].as_u64(), full["format"].as_str()), (Some("opus"), Some(48_000), Some("opus")), "{full}");
}

#[tokio::test]
async fn dsf_passes_through_when_listed_and_is_pcm_otherwise() {
    let mut e = setup("dsf").await;
    let bytes = dsf_bytes();
    std::fs::write(e.music.join("c.dsf"), &bytes).unwrap();
    let dsf = e.add("c.dsf");

    // listed (as dsf or as dsd): the file itself, with Range
    for list in ["flac,dsf,dff", "dsd", "DSF"] {
        let r = e.stream(dsf, "", Some(list)).await;
        assert_eq!((r.h("x-encore-stream").as_str(), r.h("content-type").as_str()), ("original", "audio/x-dsf"), "{list}");
        assert_eq!(r.body, bytes, "{list}");
        assert_eq!(r.h("accept-ranges"), "bytes");
    }
    let r = e.stream(dsf, "?codecs=dsf", None).await;
    assert_eq!(r.h("x-encore-stream"), "original");
    assert_eq!(e.info(dsf, "", Some("dsf")).await, json!({ "kind": "original", "codec": "dsf", "reason": null }));
    // not listed: FLAC; no list: FLAC as today (browsers can't play DSD)
    let s = e.info(dsf, "", Some("flac,opus")).await;
    assert_eq!((s["kind"].as_str(), s["reason"].as_str()), (Some("flac-fallback"), Some("the client has no DSF decoder")));
    let s = e.info(dsf, "", None).await;
    assert_eq!((s["kind"].as_str(), s["codec"].as_str(), s["reason"].as_str()), (Some("flac"), Some("dsf"), Some("browsers can't play DSF")));

    if !have_ffmpeg() {
        eprintln!("ffmpeg not found: skipping the DSD to FLAC check");
        return;
    }
    // DSD has no PCM rate: 24-bit / 176.4 kHz as today, for the fallback and for browsers alike
    let r = e.stream(dsf, "", Some("flac,opus")).await;
    assert_eq!(r.h("x-encore-stream"), "flac-fallback");
    assert_eq!(r.flac_info(), (176_400, 2, 24));
    let r = e.stream(dsf, "", None).await;
    assert_eq!(r.h("x-encore-stream"), "flac");
    assert_eq!(r.flac_info(), (176_400, 2, 24));
}

#[test]
fn audio_info_is_read_once_per_file_and_mtime() {
    let dir = std::env::temp_dir().join(format!("encore-audio-info-{}-{}", std::process::id(), crate::auth::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let p = path.to_string_lossy().into_owned();
    assert_eq!(super::audio_info(&p), json!({}), "no file, nothing kept");
    std::fs::write(&path, wav_bytes(44_100)).unwrap();
    let first = super::audio_info(&p);
    assert_eq!((first["codec"].as_str(), first["sample_rate"].as_u64(), first["bit_depth"].as_u64(), first["lossless"].as_bool()), (Some("wav"), Some(44_100), Some(16), Some(true)), "{first}");
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    // the same mtime: the kept answer, whatever the file holds now
    std::fs::write(&path, wav_bytes(96_000)).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
    assert_eq!(super::audio_info(&p)["sample_rate"], 44_100);
    // a newer mtime: read again
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime + std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(super::audio_info(&p)["sample_rate"], 96_000);
    // gone: `{}`, and the kept answer doesn't come back for a file that isn't there
    std::fs::remove_file(&path).unwrap();
    assert_eq!(super::audio_info(&p), json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_plan_by_codec_and_list() {
    let list = |s: &str| s.split(',').map(str::to_string).collect::<Vec<_>>();
    let info = |codec: &str| json!({ "codec": codec });
    // what lofty says wins over the extension; "ogg vorbis" is vorbis to the list
    assert_eq!(super::plan("m4a", &info("alac"), Some(&list("aac,flac"))).kind, "flac-fallback");
    assert_eq!(super::plan("m4a", &info("alac"), Some(&list("alac"))).kind, "original");
    assert_eq!(super::plan("ogg", &info("ogg vorbis"), Some(&list("vorbis"))).kind, "original");
    assert_eq!(super::plan("ogg", &info("ogg vorbis"), Some(&list("opus"))).reason.unwrap(), "the client has no Vorbis decoder");
    // no reading of the file: the extension
    assert_eq!(super::plan("dff", &json!({}), Some(&list("dsd"))).kind, "flac-fallback", "the list is expanded by `decoders`, not here");
    assert_eq!(super::plan("dff", &json!({}), Some(&list("dff"))).content_type, Some("audio/x-dff"));
    assert_eq!(super::plan("wv", &json!({}), Some(&list("wavpack"))).content_type, Some("audio/x-wavpack"));
    assert_eq!(super::plan("", &json!({}), Some(&list("flac"))).reason.unwrap(), "the file's codec is unknown");
    // without a list: browsers
    assert_eq!(super::plan("m4a", &info("alac"), None).reason.unwrap(), "browsers can't play ALAC");
    assert_eq!(super::plan("m4a", &info("aac"), None).content_type, Some("audio/mp4"));
    assert_eq!(super::plan("mp3", &info("mp3"), None).kind, "original");
    assert_eq!(super::plan("wv", &json!({}), None).kind, "flac");
    // the list: header first, then the query; case and spaces don't matter; dsd and ogg are expanded
    let mut h = HeaderMap::new();
    assert_eq!(super::decoders(&h, " Flac, DSD ,ogg,,"), Some(list("flac,dsf,dff,vorbis")));
    assert_eq!(super::decoders(&h, " , "), None);
    h.insert("x-encore-decoders", "opus".parse().unwrap());
    assert_eq!(super::decoders(&h, "mp3"), Some(list("opus")));
    h.insert("x-encore-decoders", "".parse().unwrap());
    assert_eq!(super::decoders(&h, "mp3"), Some(list("mp3")));
}

#[tokio::test]
async fn offline_ok_only_for_the_original_and_made_streams_are_never_stored() {
    let mut e = setup("offline").await;
    std::fs::write(e.music.join("d.wav"), wav_bytes(44_100)).unwrap();
    std::fs::write(e.music.join("e.dsf"), dsf_bytes()).unwrap();
    let wav = e.add("d.wav");
    let dsf = e.add("e.dsf");
    let info = |id: i64, query: &'static str, decoders: Option<&'static str>| {
        let e = &e;
        async move { e.get(&format!("/api/library/tracks/{id}/info{query}"), decoders).await.json() }
    };
    // `offline_ok` next to `stream`: true exactly when the stream is the original for this client
    for (id, query, decoders, kind) in [
        (wav, "", None, "original"),                     // a browser plays WAV
        (wav, "", Some("flac,wav"), "original"),
        (wav, "", Some("flac,mp3,aac"), "flac-fallback"), // no WAV decoder: a conversion
        (wav, "?codecs=wav", None, "original"),
        (dsf, "", None, "flac"),                         // browsers can't play DSD
        (dsf, "", Some("dsd"), "original"),
        (dsf, "?codecs=flac", None, "flac-fallback"),
    ] {
        let v = info(id, query, decoders).await;
        assert_eq!(v["stream"]["kind"], kind, "{id}{query} {decoders:?}: {v}");
        assert_eq!(v["offline_ok"], kind == "original", "{id}{query} {decoders:?}: {v}");
    }

    // the original keeps its header; DSF passed through too
    let r = e.stream(wav, "", None).await;
    assert_eq!((r.h("x-encore-stream").as_str(), r.h("cache-control").as_str()), ("original", "private, max-age=3600"));
    let r = e.stream(dsf, "", Some("dsd")).await;
    assert_eq!((r.h("x-encore-stream").as_str(), r.h("cache-control").as_str()), ("original", "private, max-age=3600"));
    if !have_ffmpeg() {
        eprintln!("ffmpeg not found: skipping the no-store check of made streams");
        return;
    }
    // made on the fly: the asked-for conversion, the fallback, the lossy rungs, DSD for a browser
    for (id, query, decoders, label) in [
        (wav, "?transcode=1", None, "flac"),
        (wav, "", Some("flac,mp3"), "flac-fallback"),
        (wav, "?q=aac&kbps=128", None, "aac-128"),
        (dsf, "", None, "flac"),
    ] {
        let r = e.stream(id, query, decoders).await;
        assert_eq!(r.h("x-encore-stream"), label, "{id}{query} {decoders:?}");
        assert_eq!(r.h("cache-control"), "private, no-store", "{id}{query} {decoders:?}");
    }
}
