//! How a library song is streamed. One decision, [`plan_stream`], picks the answer for every stream request
//! (a signed-in listener's or a share link's): the original file when the client plays it (codec negotiation
//! from P1: `X-Encore-Decoders` or `?codecs=`), FLAC or a lossy rung made on the fly by ffmpeg, or server
//! processing by Cadenza ([`crate::dsp`]); [`serve_stream`] carries it out. Also serving a file with Range
//! support. The handlers (the server's routes) only look the song up and say who is listening.
use crate::Ctx;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use encore_core::audio::mime as browser_type;
use encore_core::error::{err, ApiResult};
use encore_media::probe::is_alac;
use encore_media::Ffmpeg;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// `Cache-Control` of every stream made on the fly (FLAC, the fallback, the lossy rungs, Cadenza): no cache on
/// the way keeps it (the browser's, a proxy's, the service worker's). Only the original is worth keeping; a
/// client that saves a conversion for offline play does so on purpose and marks the copy "(converted)".
pub const NO_STORE: &str = "private, no-store";

/// A file with Range support (`content_type`: the file's type when given; `download_name`: sent as an attachment).
pub async fn serve_file(path: &str, req: Request, content_type: Option<&str>, download_name: Option<String>) -> ApiResult<Response> {
    let mut svc = ServeFile::new(path);
    if let Some(ct) = content_type {
        svc = ServeFile::new_with_mime(path, &ct.parse().map_err(|_| err(500, "bad mime"))?);
    }
    let mut resp = svc.oneshot(req).await.map_err(|_| err(404, "File not found"))?.map(Body::new);
    if let Some(n) = download_name {
        let v = format!("attachment; filename*=UTF-8''{}", urlencoding::encode(&n));
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&v).unwrap_or(HeaderValue::from_static("attachment")));
    }
    Ok(resp)
}

// ---- codec negotiation ----------------------------------------------------------------------------
//
// `x-encore-stream` on every stream answer says what the body is: `original` (the file, byte for byte),
// `flac` (FLAC made on the fly because a browser can't play the file, or `transcode` was asked for),
// `flac-fallback` (FLAC made on the fly because the client's decoders don't cover the file's codec),
// `aac-<kbps>` / `opus-<kbps>` (the lossy streams) and `cadenza-<rate>` (upsampled by the server).
// It is the one header a stream carries about itself. Before 0.29 four more went along that no client
// read: `x-transcoded` (the same word on made streams), `x-cadenza-path` (the signal path label),
// `x-encore-dsp` (live, rendering or cached) and `x-encore-dsp-offset` (where a processed stream starts).
// The original also says `Vary: X-Encore-Decoders`: its body depends on the client's decoder list.

/// The response header that says what a stream's body is (the words above).
pub const STREAM_HEADER: &str = "x-encore-stream";

/// Set the `x-encore-stream` label.
pub fn set_label(h: &mut HeaderMap, label: &str) {
    if let Ok(v) = HeaderValue::from_str(label) {
        h.insert(HeaderName::from_static(STREAM_HEADER), v);
    }
}

/// The request header that lists the codecs a client decodes: `X-Encore-Decoders: flac,mp3,aac,alac,opus,
/// vorbis,wav,aiff,dsf,dff` (also `wavpack`, `ape`; `dsd` stands for dsf and dff, `ogg` for vorbis). The same
/// list goes as `?codecs=` on the URL for a client that can't send headers; the header wins when both are given.
pub const DECODERS_HEADER: &str = "x-encore-decoders";

/// The codecs the client decodes, from the header or the query; None when neither says anything (an empty
/// list counts as nothing said): the client then decodes what a browser does.
pub fn decoders(headers: &axum::http::HeaderMap, query: &str) -> Option<Vec<String>> {
    let raw = headers.get(DECODERS_HEADER).and_then(|v| v.to_str().ok()).filter(|s| !s.trim().is_empty()).unwrap_or(query);
    let mut out = Vec::new();
    for t in raw.split(',').map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()) {
        match t.as_str() {
            "dsd" => out.extend(["dsf".to_string(), "dff".to_string()]),
            "ogg" => out.push("vorbis".to_string()),
            _ => out.push(t),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// What the file holds, in the words of the decoders list: from `audio_info` (which tells ALAC from AAC in
/// .m4a and Opus from Vorbis in .ogg), or from the extension for what lofty doesn't read (DSF, DFF).
fn file_codec(fmt: &str, info: &Value) -> String {
    match info["codec"].as_str().unwrap_or("") {
        "" => encore_core::audio::codec_of_ext(fmt).to_string(),
        "ogg vorbis" => "vorbis".to_string(),
        c => c.to_string(),
    }
}

/// The codec as the signal path spells it.
fn codec_name(codec: &str) -> String {
    match codec { "opus" => "Opus".to_string(), "vorbis" => "Vorbis".to_string(), "wavpack" => "WavPack".to_string(), "speex" => "Speex".to_string(), c => c.to_uppercase() }
}

/// The content type of an original a client asked for by codec (browsers never get the ones past `browser_type`).
fn original_type(fmt: &str) -> &'static str {
    encore_core::audio::content_type(fmt).unwrap_or("application/octet-stream")
}

/// How a plain stream of a file (no lossy, Cadenza or `transcode` switch) is served, decided the same way for
/// the stream and for `/tracks/{id}/info`.
pub struct Plan {
    /// the `x-encore-stream` label: `original`, `flac` or `flac-fallback`
    pub kind: &'static str,
    /// the file's codec: `opus`, `flac`, `alac`, `dsf` …
    pub codec: String,
    /// why the body is not the file, in plain words (None for the original)
    pub reason: Option<String>,
    /// the original's content type; None when FLAC is made on the fly
    pub content_type: Option<&'static str>,
}

/// With a decoders list: the original when the file's codec is listed (a FLAC file always: the fallback
/// would be FLAC too), else `flac-fallback`. Without one: the original when a browser plays the format, else `flac`.
pub fn plan(fmt: &str, info: &Value, decoders: Option<&[String]>) -> Plan {
    let codec = file_codec(fmt, info);
    let original = |ct| Plan { kind: "original", codec: codec.clone(), reason: None, content_type: Some(ct) };
    match decoders {
        Some(list) if codec == "flac" || list.contains(&codec) => original(original_type(fmt)),
        Some(_) => {
            let reason = if codec.is_empty() { "the file's codec is unknown".to_string() } else { format!("the client has no {} decoder", codec_name(&codec)) };
            Plan { kind: "flac-fallback", codec, reason: Some(reason), content_type: None }
        }
        None => match browser_type(fmt) {
            Some(ct) if codec != "alac" => original(ct),
            _ => Plan { kind: "flac", reason: Some(format!("browsers can't play {}", codec_name(&codec))), codec, content_type: None },
        },
    }
}


// ---- the stream decision ----------------------------------------------------------------------------

/// What a stream request asks for: the query of `/api/library/tracks/{id}/stream` and `/api/share/{token}/stream`.
#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct StreamQ {
    /// convert to FLAC (formats a browser can't play); `1/true/yes/on` like `dsp`
    #[serde(deserialize_with = "flag")]
    pub transcode: bool,
    /// start a converted stream at this many seconds (the player seeks converted streams through the server)
    pub t: f64,
    /// "aac"/"opus": a lossy stream at `kbps` (Auto's adaptive rungs 256–32, or Data saver's 96)
    pub q: String,
    /// bitrate of the lossy stream in kbps (32–320, default 256; the player steps down to 32 when the connection is slow)
    pub kbps: u32,
    /// upsample on the server with Cadenza (admin switch + `server_dsp` permission; never on share links)
    #[serde(deserialize_with = "flag")]
    pub dsp: bool,
    /// Cadenza output rate in Hz (0 = the highest multiple of the source rate the server allows)
    pub rate: u32,
    /// Cadenza filter id (empty = server default)
    pub filter: String,
    /// Cadenza dither id (default TPDF)
    pub dither: String,
    /// the codecs this client decodes, comma separated (`X-Encore-Decoders` as a query, for an `<audio>` element
    /// that can't send headers): the original when the file's codec is listed, else lossless FLAC
    pub codecs: String,
}

/// A query switch: `1`, `true`, `yes` or `on` (an empty or missing value is off).
fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// A signed-in listener. A share link has none, and its streams are never processed on the server.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    /// the user's id
    pub user: i64,
    /// whether the listener has the `server_dsp` permission (only asked when the request wants processing)
    pub may_process: bool,
}

/// How one stream request is answered.
#[derive(Debug, PartialEq)]
pub enum StreamPlan {
    /// the file itself with Range support (`original`)
    Original {
        /// the file's Content-Type
        content_type: &'static str,
    },
    /// made on the fly by ffmpeg, streamed without a length
    Convert(Convert),
    /// upsampled on the server by Cadenza for this user (`cadenza-<rate>`): the render cache or live
    Cadenza {
        /// the user it is processed for
        user: i64,
    },
}

/// A stream ffmpeg makes on the fly.
#[derive(Debug, PartialEq)]
pub enum Convert {
    /// lossless FLAC: `flac` (a browser can't play the file, or `transcode` was asked for) or, with
    /// `fallback`, `flac-fallback` (the client's decoders don't cover the file's codec); DSD (`dsd`) is
    /// decoded to 24-bit / 176.4 kHz first
    Flac {
        /// the client's decoders don't cover the file's codec
        fallback: bool,
        /// the file is DSD, decoded to PCM first
        dsd: bool,
    },
    /// AAC in an ADTS stream at this many kbps (`aac-<kbps>`)
    Aac(u32),
    /// Opus in Ogg at this many kbps (`opus-<kbps>`), for browsers without an AAC decoder
    Opus(u32),
}

impl Convert {
    /// The `x-encore-stream` label.
    pub fn label(&self) -> String {
        match self {
            Convert::Flac { fallback: true, .. } => "flac-fallback".into(),
            Convert::Flac { .. } => "flac".into(),
            Convert::Aac(kbps) => format!("aac-{kbps}"),
            Convert::Opus(kbps) => format!("opus-{kbps}"),
        }
    }

    /// The Content-Type of the stream.
    pub fn content_type(&self) -> &'static str {
        match self {
            Convert::Flac { .. } => "audio/flac",
            Convert::Aac(_) => "audio/aac",
            Convert::Opus(_) => "audio/ogg",
        }
    }

    /// ffmpeg's arguments after the input: the filter, the encoder and the container, to stdout.
    fn args(&self) -> Vec<String> {
        let a: Vec<&str> = match self {
            // DSD has no PCM sample rate: decode it to 24-bit / 176.4 kHz, the usual DSD → PCM choice and a rate browsers play
            Convert::Flac { dsd: true, .. } => vec!["-af", "aresample=176400", "-sample_fmt", "s32", "-c:a", "flac", "-compression_level", "0", "-f", "flac"],
            Convert::Flac { .. } => vec!["-c:a", "flac", "-compression_level", "0", "-f", "flac"],
            // starts fast and needs no container index
            Convert::Aac(_) => vec!["-c:a", "aac", "-f", "adts"],
            Convert::Opus(_) => vec!["-c:a", "libopus", "-vbr", "on", "-application", "audio", "-f", "ogg"],
        };
        let mut a: Vec<String> = a.into_iter().map(String::from).collect();
        if let Convert::Aac(kbps) | Convert::Opus(kbps) = self {
            // the bitrate right after the codec, as ffmpeg's own examples write it
            a.splice(2..2, ["-b:a".to_string(), format!("{kbps}k")]);
        }
        a.extend(["-flush_packets", "1", "pipe:1"].map(String::from));
        a
    }
}

/// The one decision for a stream of the library file `path` (format `fmt` as the scan stored it):
/// - the lossy stream (Data saver, Auto's lighter rungs) wins over server processing;
/// - server processing for a signed-in listener who asks for it (403 when the server has it off or the
///   listener may not use it; a share link never gets it);
/// - a conversion the client asked for (`transcode`) is FLAC;
/// - a client that says what it decodes gets the original when the file's codec is listed (DSF and DFF
///   included), lossless FLAC (`flac-fallback`) otherwise;
/// - any other client is a browser: the original when browsers play the format (ALAC in .m4a they don't),
///   else FLAC.
pub async fn plan_stream(st: &impl Ctx, path: &str, fmt: &str, q: &StreamQ, headers: &HeaderMap, listener: Option<Listener>) -> ApiResult<StreamPlan> {
    let lossy = matches!(q.q.as_str(), "aac" | "lossy" | "opus");
    if let (true, false, Some(l)) = (q.dsp, lossy, listener) {
        if !crate::dsp::Limits::of(st).enabled {
            return Err(err(403, "Server processing is turned off on this server"));
        }
        if !l.may_process {
            return Err(err(403, "Your account can't use server processing"));
        }
        return Ok(StreamPlan::Cadenza { user: l.user });
    }
    if lossy {
        let kbps = if q.kbps == 0 { 256 } else { q.kbps.clamp(32, 320) };
        return Ok(StreamPlan::Convert(if q.q == "opus" { Convert::Opus(kbps) } else { Convert::Aac(kbps) }));
    }
    let dsd = matches!(fmt, "dsf" | "dff");
    if q.transcode {
        return Ok(StreamPlan::Convert(Convert::Flac { fallback: false, dsd }));
    }
    if let Some(list) = decoders(headers, &q.codecs) {
        let (p2, media) = (path.to_string(), st.media().clone());
        let info = tokio::task::spawn_blocking(move || media.audio_info(&p2)).await.unwrap_or(json!({}));
        return Ok(match plan(fmt, &info, Some(&list)).content_type {
            Some(content_type) => StreamPlan::Original { content_type },
            None => StreamPlan::Convert(Convert::Flac { fallback: true, dsd }),
        });
    }
    let alac = if fmt == "m4a" || fmt == "mp4" {
        let p2 = path.to_string();
        tokio::task::spawn_blocking(move || is_alac(&p2)).await.unwrap_or(false)
    } else {
        false
    };
    Ok(match browser_type(fmt) {
        Some(content_type) if !alac => StreamPlan::Original { content_type },
        _ => StreamPlan::Convert(Convert::Flac { fallback: false, dsd }),
    })
}

/// A stream of library song `track_id` (file `path`, format `fmt`), as [`plan_stream`] decides for `listener`
/// (None for a share link).
pub async fn serve_stream(st: &impl Ctx, track_id: i64, path: &str, fmt: &str, q: &StreamQ, req: Request, listener: Option<Listener>) -> ApiResult<Response> {
    if !std::path::Path::new(path).is_file() {
        return Err(err(404, "Track not found"));
    }
    match plan_stream(st, path, fmt, q, req.headers(), listener).await? {
        StreamPlan::Original { content_type } => original(path, req, content_type).await,
        StreamPlan::Convert(c) => convert(path, q.t, &c),
        StreamPlan::Cadenza { user } => crate::dsp::serve(st, track_id, path.to_string(), q, user, req).await,
    }
}

/// The file itself (Range requests work), labelled `original`.
pub async fn original(path: &str, req: Request, content_type: &str) -> ApiResult<Response> {
    let mut r = serve_file(path, req, Some(content_type), None).await?;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    // the body depends on the client's decoder list (an Opus file is the original for one client and a FLAC pipe
    // for another), so a cache in front of the client keeps the answers apart
    r.headers_mut().insert(header::VARY, HeaderValue::from_static("X-Encore-Decoders"));
    set_label(r.headers_mut(), "original");
    Ok(r)
}

/// `path` made into `c` by ffmpeg from `seek` seconds on. The ffmpeg process is killed when the response
/// (and with it the stream) is dropped. It runs at normal priority on purpose: someone is listening to it (a
/// group listen's member too), while every background ffmpeg (loudness, the import's audio check, the song
/// edges) runs at idle priority ([`encore_media::Ffmpeg::low_priority`]).
pub fn convert(path: &str, seek: f64, c: &Convert) -> ApiResult<Response> {
    let mut child = Ffmpeg::new().quiet().seek(seek).input(path).args(["-map", "0:a:0"]).args(c.args()).pipe_out().spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
    let out = child.stdout.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;
    let stream = futures_util::StreamExt::map(tokio_util::io::ReaderStream::new(out), move |c| {
        let _keep = &child;
        c
    });
    let mut r = ([(header::CONTENT_TYPE, c.content_type())], Body::from_stream(stream)).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    set_label(r.headers_mut(), &c.label());
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_keep_their_ffmpeg_arguments_and_labels() {
        let tail = ["-flush_packets", "1", "pipe:1"];
        let f = Convert::Flac { fallback: false, dsd: false };
        assert_eq!(f.args(), [&["-c:a", "flac", "-compression_level", "0", "-f", "flac"][..], &tail].concat());
        assert_eq!((f.label().as_str(), f.content_type()), ("flac", "audio/flac"));
        let d = Convert::Flac { fallback: true, dsd: true };
        assert_eq!(d.args(), [&["-af", "aresample=176400", "-sample_fmt", "s32", "-c:a", "flac", "-compression_level", "0", "-f", "flac"][..], &tail].concat());
        assert_eq!(d.label(), "flac-fallback");
        let a = Convert::Aac(128);
        assert_eq!(a.args(), [&["-c:a", "aac", "-b:a", "128k", "-f", "adts"][..], &tail].concat());
        assert_eq!((a.label().as_str(), a.content_type()), ("aac-128", "audio/aac"));
        let o = Convert::Opus(96);
        assert_eq!(o.args(), [&["-c:a", "libopus", "-b:a", "96k", "-vbr", "on", "-application", "audio", "-f", "ogg"][..], &tail].concat());
        assert_eq!((o.label().as_str(), o.content_type()), ("opus-96", "audio/ogg"));
    }

    #[test]
    fn query_switches() {
        let q: StreamQ = serde_json::from_value(json!({ "dsp": "on", "transcode": "0", "kbps": 128 })).unwrap();
        assert!(q.dsp && !q.transcode);
        assert_eq!(q.kbps, 128);
        for (raw, on) in [("1", true), ("TRUE", true), (" yes ", true), ("off", false), ("", false)] {
            let q: StreamQ = serde_json::from_value(json!({ "transcode": raw })).unwrap();
            assert_eq!(q.transcode, on, "{raw:?}");
        }
    }
}
