//! Encore library tracks (`{id, title, artist, album, album_id, format, duration}`) as engine
//! queue entries streaming from the server, the shape cadenza's own Encore client builds
//! (crates/cadenza/src/encore/commands.rs `track_from_encore`), plus the header the server's
//! codec negotiation reads, and the query the account's playback settings add to a stream
//! (Data saver, the server's Cadenza), as web-rs/src/player/load.rs `stream_url` builds it.

use super::Track;
use cadenza::decode::Source;
use serde_json::Value;
use std::sync::RwLock;

/// The query every library stream gets now (`q=aac&kbps=96`, `dsp=1&rate=…`, or empty for the
/// original file). The player keeps it in step with the account's playback settings.
static QUERY: RwLock<String> = RwLock::new(String::new());

/// The stream query for these playback settings (`prefs.playback` of the account), the server's
/// `/api/playback/dsp` answer and this device's output rate (0 when not known yet):
/// Data saver asks for a 96 kbps AAC stream; "Upsample my streams on the server", when the
/// server allows it, asks for its Cadenza at the chosen rate, filter and dither. Auto plays the
/// original here: the engine does not step the stream down by bandwidth.
pub fn stream_query(prefs: &Value, dsp: &Value, device_rate: u32) -> String {
    if prefs["quality"].as_str() == Some("saver") {
        return "q=aac&kbps=96".into();
    }
    if prefs["dsp"] != Value::Bool(true) || !crate::fmt::truthy(dsp, "allowed") {
        return String::new();
    }
    let rate = match &prefs["dsp_rate"] {
        Value::String(s) if s == "device" || s.is_empty() => u64::from(device_rate),
        Value::String(s) if s == "auto" => 0,
        v => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .filter(|r| r.is_finite() && *r > 0.0)
            .map(|r| r as u64)
            .unwrap_or(0),
    };
    let enc = crate::pages::urlencode;
    let mut q = vec!["dsp=1".to_string(), format!("rate={rate}")];
    match prefs["dsp_filter"].as_str() {
        Some(f) if !f.is_empty() => q.push(format!("filter={}", enc(f))),
        _ => {}
    }
    let dither = prefs["dsp_dither"].as_str().filter(|d| !d.is_empty()).unwrap_or("tpdf");
    q.push(format!("dither={}", enc(dither)));
    q.join("&")
}

/// Set the query of the streams queued from now on.
pub fn set_query(q: String) {
    *QUERY.write().unwrap_or_else(|e| e.into_inner()) = q;
}

fn query() -> String {
    QUERY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What this engine decodes (symphonia with every codec, plus cadenza's DSD readers), in the
/// words of the server's `X-Encore-Decoders` list (server/src/routes/library.rs).
pub const DECODERS: &str = "flac,mp3,aac,alac,vorbis,wav,aiff,dsf,dff";

/// An auth header for the stream: `("Authorization", "Bearer <app token>")`, or the session
/// cookie before a token exists.
pub type Auth = (String, String);

pub fn bearer(token: &str) -> Auth {
    ("Authorization".into(), format!("Bearer {token}"))
}

pub fn cookie(session_cookie: &str) -> Auth {
    ("Cookie".into(), format!("{}={session_cookie}", crate::api::COOKIE))
}

/// The request headers of a stream: the credential and the decoders list.
pub fn stream_headers(auth: &Auth) -> Vec<(String, String)> {
    vec![auth.clone(), ("X-Encore-Decoders".into(), DECODERS.into())]
}

/// `<base>/api/library/tracks/{id}/stream`: the original file (what Save for offline keeps).
pub fn stream_url(base: &str, id: i64) -> String {
    format!("{base}/api/library/tracks/{id}/stream")
}

/// The stream to play: the original, or what the playback settings ask for (`?q=…`, `?dsp=1…`).
fn play_url(base: &str, id: i64) -> String {
    let q = query();
    if q.is_empty() {
        stream_url(base, id)
    } else {
        format!("{}?{q}", stream_url(base, id))
    }
}

/// A track id from a JSON row (number or numeric string).
pub fn id_of(v: &Value) -> Option<i64> {
    v.get("id")
        .and_then(|i| i.as_i64().or_else(|| i.as_str().and_then(|s| s.parse().ok())))
}

/// One Encore track as a queue entry.
pub fn from_encore(base: &str, auth: &Auth, v: &Value) -> Option<Track> {
    let id = id_of(v)?;
    let album_id = v
        .get("album_id")
        .and_then(|a| {
            a.as_str()
                .map(str::to_string)
                .or_else(|| a.as_i64().map(|n| n.to_string()))
        })
        .filter(|a| !a.is_empty());
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let artist = {
        let a = s("artist");
        if a.is_empty() {
            s("albumartist")
        } else {
            a
        }
    };
    Some(Track {
        id: Some(id.to_string()),
        source: Source::Url {
            url: play_url(base, id),
            headers: stream_headers(auth),
        },
        title: s("title"),
        artist,
        album: s("album"),
        album_id: album_id.clone(),
        duration: v.get("duration").and_then(Value::as_f64),
        format: Some(crate::fmt::quality(v)).filter(|q| !q.is_empty()),
        cover: album_id
            .as_deref()
            .map(|a| crate::widgets::cover::album_url(base, a, &v["cover_v"], 600)),
    })
}

/// A list of Encore tracks (rows without an id are skipped).
pub fn many(base: &str, auth: &Auth, rows: &[Value]) -> Vec<Track> {
    rows.iter().filter_map(|t| from_encore(base, auth, t)).collect()
}

/// An offline song from disk.
pub fn from_file(path: std::path::PathBuf, v: &Value) -> Track {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Track {
        id: id_of(v).map(|i| i.to_string()),
        source: Source::Path { path },
        title: s("title"),
        artist: s("artist"),
        album: s("album"),
        album_id: v.get("album_id").and_then(Value::as_str).map(str::to_string),
        duration: v.get("duration").and_then(Value::as_f64),
        format: v.get("format").and_then(Value::as_str).map(str::to_string),
        cover: v.get("art").and_then(Value::as_str).map(str::to_string),
    }
}

/// The Encore track id of a queue entry (None for files and other sources).
pub fn encore_id(t: &Track) -> Option<i64> {
    let id = t.id.as_deref()?.parse::<i64>().ok()?;
    match &t.source {
        Source::Url { url, .. } if url.contains("/api/library/tracks/") => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encore_rows_become_streaming_entries_with_the_decoders_header() {
        let auth = bearer("tok");
        let t = from_encore(
            "http://e:8080",
            &auth,
            &serde_json::json!({ "id": 12, "title": "Song", "artist": "Band", "album": "LP", "album_id": "cc5d7f35", "format": "flac", "duration": 200.5 }),
        )
        .unwrap();
        assert_eq!(t.id.as_deref(), Some("12"));
        assert_eq!(t.format.as_deref(), Some("FLAC"));
        assert_eq!(t.cover.as_deref(), Some("http://e:8080/api/library/albums/cc5d7f35/cover?s=600"));
        match &t.source {
            Source::Url { url, headers } => {
                assert_eq!(url, "http://e:8080/api/library/tracks/12/stream");
                assert_eq!(headers[0], ("Authorization".to_string(), "Bearer tok".to_string()));
                assert_eq!(headers[1], ("X-Encore-Decoders".to_string(), DECODERS.to_string()));
            }
            _ => panic!("expected a URL"),
        }
        assert_eq!(encore_id(&t), Some(12));
        assert!(from_encore("http://e", &auth, &serde_json::json!({ "title": "no id" })).is_none());
        assert_eq!(cookie("abc").1, "encore_session=abc");
    }

    #[test]
    fn playback_settings_become_the_web_players_stream_query() {
        use serde_json::json;
        let allowed = json!({ "enabled": true, "allowed": true });
        assert_eq!(stream_query(&json!({ "quality": "auto" }), &allowed, 48000), "");
        assert_eq!(stream_query(&json!({ "quality": "saver", "dsp": true }), &allowed, 48000), "q=aac&kbps=96");
        let dsp = json!({ "quality": "original", "dsp": true, "dsp_rate": "device", "dsp_filter": "", "dsp_dither": "tpdf" });
        assert_eq!(stream_query(&dsp, &allowed, 96000), "dsp=1&rate=96000&dither=tpdf");
        // not allowed (or not known yet): the original, never a request the server refuses
        assert_eq!(stream_query(&dsp, &json!({ "enabled": true, "allowed": false }), 96000), "");
        assert_eq!(stream_query(&dsp, &Value::Null, 96000), "");
        let fixed = json!({ "dsp": true, "dsp_rate": 192000, "dsp_filter": "sinc-m", "dsp_dither": "ns 5" });
        assert_eq!(stream_query(&fixed, &allowed, 0), "dsp=1&rate=192000&filter=sinc%2Dm&dither=ns%205");
        assert_eq!(stream_query(&json!({ "dsp": true, "dsp_rate": "auto" }), &allowed, 44100), "dsp=1&rate=0&dither=tpdf");
    }
}
