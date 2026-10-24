//! Saving songs on this device: the audio (the original file when this browser plays it, else a lossy copy
//! the server converts, marked "(converted)"), the album cover at 600 px and the lyrics, two songs at a time.
//! The body is read in pieces so the progress counts bytes; the pieces become one Blob that goes into the
//! cache as a synthetic answer (store.rs). A song that fails is reported and the others go on – except after
//! the device ran out of room, when the rest fail at once instead of being fetched for nothing.

use super::store::{self, Record};
use crate::api;
use crate::json::J;
use futures::StreamExt;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, Cache, Response};

/// kbit/s of the converted copy (AAC, or Opus where the browser has no AAC decoder).
pub const CONVERTED_KBPS: i64 = 256;
/// Songs fetched at the same time.
const AT_ONCE: usize = 2;
/// No data for this long: the download stops.
const STALL_MS: u32 = 60_000;
/// The pieces of a body are joined into a Blob every few MB, which the browser may keep on disk.
const PART_BYTES: f64 = 4.0 * 1024.0 * 1024.0;
/// The `/info` of a song gets this long: a server that stopped answering must not hold a download slot.
const INFO_WAIT_MS: u32 = 20_000;

/// How far a [`download`] is: `done` of `total` songs finished, `bytes` received so far, `current` the title
/// of the song being fetched.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub bytes: f64,
    pub current: String,
}

/// How a [`download`] went: songs saved, songs that were already saved (or being saved by another download),
/// and the failures with their reason in plain words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub saved: usize,
    pub skipped: usize,
    pub failed: Vec<(i64, String)>,
}

thread_local! {
    /// Songs some download is fetching right now.
    static ACTIVE: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
}

/// Whether some download is saving songs on this device right now (a reload would stop it; update.rs waits).
pub fn downloading() -> bool { ACTIVE.with(|a| !a.borrow().is_empty()) }

/// The library id of a song row (None for previews and catalog rows).
pub fn track_id(row: &Value) -> Option<i64> {
    if row.truthy("preview") { return None; }
    let id = row.g("id");
    id.as_i64().or_else(|| id.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)).filter(|id| *id > 0)
}

/// Save these songs (library rows as the server sends them) on this device. Songs already saved are skipped;
/// rows without a library id are left out.
pub async fn download(tracks: &[Value], on_progress: impl Fn(Progress)) -> Summary { run(tracks, on_progress, false).await }

/// "Download again in original quality": like [`download`], but songs already saved are fetched again and
/// replace their copy (the old one stays until the new one is complete) – only where the original can be
/// saved now; a song whose original still can't is not fetched and counts as skipped.
pub async fn download_again(tracks: &[Value], on_progress: impl Fn(Progress)) -> Summary { run(tracks, on_progress, true).await }

async fn run(tracks: &[Value], on_progress: impl Fn(Progress), again: bool) -> Summary {
    store::index().await;
    let mut summary = Summary::default();
    let mut seen = HashSet::new();
    let mut jobs: Vec<(i64, &Value)> = vec![];
    for row in tracks {
        let Some(id) = track_id(row) else { continue };
        if !seen.insert(id) { continue; }
        if (!again && store::has(id)) || ACTIVE.with(|a| a.borrow().contains(&id)) { summary.skipped += 1; continue; }
        jobs.push((id, row));
    }
    if jobs.is_empty() { return summary; }
    if !store::available() {
        summary.failed = jobs.iter().map(|(id, _)| (*id, store::NO_STORE.to_string())).collect();
        return summary;
    }
    ACTIVE.with(|a| a.borrow_mut().extend(jobs.iter().map(|(id, _)| *id)));
    let progress = RefCell::new(Progress { total: jobs.len(), ..Default::default() });
    let summary = RefCell::new(summary);
    let full = Cell::new(false);
    let last_emit = Cell::new(0.0);
    let emit = |force: bool| {
        let now = crate::dom::now_ms();
        if !force && now - last_emit.get() < 200.0 { return; }
        last_emit.set(now);
        let p = progress.borrow().clone();
        on_progress(p);
    };
    emit(true);
    futures::stream::iter(jobs)
        .for_each_concurrent(AT_ONCE, |(id, row)| {
            let (progress, summary, full, emit) = (&progress, &summary, &full, &emit);
            async move {
                let r = if full.get() {
                    Err(store::QUOTA_FULL.to_string())
                } else {
                    progress.borrow_mut().current = row.s("title");
                    emit(true);
                    let on_bytes = |n: f64| { progress.borrow_mut().bytes += n; emit(false); };
                    save_one(id, row, &on_bytes, again).await
                };
                ACTIVE.with(|a| a.borrow_mut().remove(&id));
                match r {
                    Ok(true) => summary.borrow_mut().saved += 1,
                    // "Download again", but the original still can't be saved here: the copy stays as it is
                    Ok(false) => summary.borrow_mut().skipped += 1,
                    Err(e) => {
                        if e == store::QUOTA_FULL { full.set(true); }
                        crate::dom::console_error(&format!("download of song {id} failed: {e}"));
                        summary.borrow_mut().failed.push((id, e));
                    }
                }
                progress.borrow_mut().done += 1;
                emit(true);
            }
        })
        .await;
    summary.into_inner()
}

/// One song: what to fetch, the audio (kept in memory), the cover and lyrics, then the audio goes into the
/// cache right before its record, so a page closed in the middle leaves no audio without a record (what it
/// leaves goes at the next sweep, store.rs). Whatever it put in the cache goes again when it fails.
/// `again` ("Download again in original quality"): when the original still can't be saved here, nothing is
/// fetched and the saved copy stays (`Ok(false)`).
async fn save_one(id: i64, row: &Value, on_bytes: &dyn Fn(f64), again: bool) -> Result<bool, String> {
    let info = api::get_within(&format!("/library/tracks/{id}/info"), INFO_WAIT_MS).await.map_err(|e| e.message)?;
    let format = { let f = row.s("format"); if f.is_empty() { info.s("format") } else { f } }.to_ascii_lowercase();
    let lossy = if offers_original(&info) && crate::player::can_play(&format) { None } else { Some(lossy_codec()) };
    if again && lossy.is_some() && store::has(id) { return Ok(false); }
    let cache = store::open().await?;
    let got = fetch_audio(&stream_url(id, lossy), on_bytes).await?;
    let ctype = if got.ctype.is_empty() { mime_for(lossy.unwrap_or(&format)).to_string() } else { got.ctype.clone() };
    let album_id = store::clean_id(&row.s("album_id"));
    let (cover, new_cover) = if album_id.is_empty() { (false, false) } else { save_cover(&cache, &album_id).await };
    let lyrics = save_lyrics(&cache, id).await;
    let kept = Kept::of(lossy, &got.label, &format, &info, row);
    let origin = crate::dom::location().origin().unwrap_or_default();
    let rec = build_record(row, &info, &kept, got.bytes, cover, lyrics, crate::dom::now_s() as i64, &origin);
    let audio = store::audio_key(id);
    let saved = match store::write_blob(&cache, &audio, &got.blob, &ctype).await {
        Ok(()) => store::add_record(&cache, rec).await,
        Err(e) => Err(e),
    };
    if let Err(e) = saved {
        // a song saved before ("Download again") keeps its record and the audio the cache holds for it
        if !store::has(id) {
            store::delete(&cache, &audio).await;
            store::delete(&cache, &store::lyrics_key(id)).await;
        }
        if new_cover && !store::index().await.iter().any(|r| r.cover && r.album_id == album_id) {
            store::delete(&cache, &store::cover_key(&album_id)).await;
        }
        return Err(e);
    }
    Ok(true)
}

/// Once per page, after sign-in: what interrupted downloads left in the cache without a record goes
/// (store.rs `sweep`; the songs this page is downloading stay).
pub async fn sweep_leftovers() {
    let busy = ACTIVE.with(|a| a.borrow().clone());
    let n = store::sweep(&busy).await;
    if n > 0 { crate::dom::console_error(&format!("offline store: removed {n} files left by interrupted downloads")); }
}

/// `/info` says the plain stream is the original file (`offline_ok`, or `stream.kind == "original"`).
pub fn offers_original(info: &Value) -> bool {
    info.truthy("offline_ok") || info.g("stream").s("kind") == "original"
}

/// The lossy codec of a converted copy: AAC where the browser decodes it, else Opus (the player's rule).
fn lossy_codec() -> &'static str {
    if crate::player::can_play("aac") { "aac" } else if crate::player::can_play("opus") { "opus" } else { "aac" }
}

/// The original (`lossy` None), or the server's lossy conversion at [`CONVERTED_KBPS`].
pub fn stream_url(id: i64, lossy: Option<&str>) -> String {
    match lossy {
        None => format!("/api/library/tracks/{id}/stream"),
        Some(codec) => format!("/api/library/tracks/{id}/stream?q={codec}&kbps={CONVERTED_KBPS}"),
    }
}

/// The content type of a saved file when the server sent none.
pub fn mime_for(format: &str) -> &'static str {
    match format {
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "m4a" | "mp4" | "alac" => "audio/mp4", "aac" => "audio/aac",
        "ogg" | "oga" | "opus" | "vorbis" => "audio/ogg", "wav" => "audio/wav", "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

/// A codec as the player writes it.
pub fn codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "opus" => "Opus".into(),
        "vorbis" | "ogg vorbis" | "ogg" | "oga" => "Vorbis".into(),
        "wavpack" | "wv" => "WavPack".into(),
        "m4a" | "mp4" => "AAC".into(),
        c => c.to_uppercase(),
    }
}

fn lossless_codec(codec: &str) -> bool { matches!(codec, "flac" | "alac" | "wav" | "aiff" | "aif" | "ape" | "wavpack" | "wv" | "tta") }

/// The quality label of an original: "FLAC 16/44.1", "FLAC 44.1 kHz", "MP3 320k", "Opus".
pub fn quality_label(codec: &str, lossless: bool, bits: i64, rate_hz: f64, kbps: f64) -> String {
    let name = codec_name(codec);
    if lossless || lossless_codec(&codec.to_ascii_lowercase()) {
        return match (bits > 0, rate_hz > 0.0) {
            (true, true) => format!("{name} {bits}/{}", crate::num::khz(rate_hz)),
            (false, true) => format!("{name} {} kHz", crate::num::khz(rate_hz)),
            (true, false) => format!("{name} {bits}-bit"),
            _ => name,
        };
    }
    if kbps > 0.0 { format!("{name} {}k", kbps.round() as i64) } else { name }
}

/// The label of a copy the server converted: "AAC 256k (converted)".
pub fn converted_label(codec: &str, kbps: i64) -> String { format!("{} {kbps}k (converted)", codec_name(codec)) }

/// What was saved: the file's extension, its codec and the quality label.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub ext: String,
    pub format: String,
    pub quality: String,
}

impl Kept {
    /// `lossy`: the codec of the conversion asked for (None = the original) · `label`: the stream's
    /// `x-encore-stream` header · `format`: the song's file format · `info`: the `/info` answer.
    pub fn of(lossy: Option<&str>, label: &str, format: &str, info: &Value, row: &Value) -> Kept {
        if let Some(codec) = lossy {
            let ext = if codec == "opus" { "opus" } else { "aac" };
            return Kept { ext: ext.into(), format: codec.into(), quality: converted_label(codec, CONVERTED_KBPS) };
        }
        let label = label.trim();
        if !label.is_empty() && label != "original" {
            // the server converted it after all (`flac`, `flac-fallback`): lossless FLAC made on the fly
            return Kept { ext: "flac".into(), format: "flac".into(), quality: format!("{} (converted)", codec_name("flac")) };
        }
        let ext: String = store::clean_id(format).to_ascii_lowercase();
        let codec = match info.s("codec").to_ascii_lowercase().as_str() {
            "" => match ext.as_str() { "m4a" | "mp4" => "aac".to_string(), "ogg" | "oga" => "vorbis".to_string(), e => e.to_string() },
            "ogg vorbis" => "vorbis".to_string(),
            c => c.to_string(),
        };
        let kbps = if info.f("bitrate") > 0.0 { info.f("bitrate") } else { row.f("bitrate") };
        let quality = quality_label(&codec, info.truthy("lossless"), info.i("bit_depth"), info.f("sample_rate"), kbps);
        Kept { ext: if ext.is_empty() { codec.clone() } else { ext }, format: codec, quality }
    }
}

/// The record of a saved song, from the library row and the `/info` answer.
#[allow(clippy::too_many_arguments)]
pub fn build_record(row: &Value, info: &Value, kept: &Kept, bytes: f64, cover: bool, lyrics: bool, now: i64, origin: &str) -> Record {
    let artist = { let a = row.s("artist"); if a.trim().is_empty() { row.s("albumartist") } else { a } };
    let duration = if row.f("duration") > 0.0 { row.f("duration") } else { info.f("duration") };
    Record {
        id: track_id(row).unwrap_or(0),
        title: row.s("title"),
        artist,
        album: row.s("album"),
        album_id: store::clean_id(&row.s("album_id")),
        ext: kept.ext.clone(),
        bytes: if bytes > 0.0 { bytes as u64 } else { 0 },
        quality: kept.quality.clone(),
        cover,
        lyrics,
        saved_at: now,
        source_server: origin.trim_end_matches('/').to_string(),
        duration: if duration.is_finite() && duration > 0.0 { duration } else { 0.0 },
        track: row.i("track").max(0),
        disc: row.i("disc").max(0),
        format: kept.format.clone(),
        artists: row.a("artists"),
    }
}

/// A failed answer in plain words.
pub fn status_words(status: u16) -> String {
    match status {
        0 => "Not connected to the server".into(),
        401 | 403 => "Not signed in to the server".into(),
        404 => "The song is not on the server any more".into(),
        s => format!("The server answered {s}"),
    }
}

// ---- fetching --------------------------------------------------------------------------------------

/// A promise, or None when it took longer than `ms`.
async fn timed(p: js_sys::Promise, ms: u32) -> Option<Result<JsValue, JsValue>> {
    let a = Box::pin(wasm_bindgen_futures::JsFuture::from(p));
    let b = Box::pin(gloo_timers::future::TimeoutFuture::new(ms));
    match futures::future::select(a, b).await {
        futures::future::Either::Left((r, _)) => Some(r),
        futures::future::Either::Right(_) => None,
    }
}

/// `fetch(url)` with the session cookie, stopped through `ctl` when it hangs.
async fn fetch(url: &str, ctl: &web_sys::AbortController) -> Result<Response, String> {
    let init = web_sys::RequestInit::new();
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_signal(Some(&ctl.signal()));
    let p = crate::dom::window().fetch_with_str_and_init(url, &init);
    match timed(p, STALL_MS).await {
        Some(Ok(r)) => {
            let r: Response = r.unchecked_into();
            if r.ok() { Ok(r) } else { Err(status_words(r.status())) }
        }
        Some(Err(_)) => Err(status_words(0)),
        None => { ctl.abort(); Err("The server stopped answering".into()) }
    }
}

struct Fetched {
    blob: Blob,
    ctype: String,
    /// `x-encore-stream`: `original`, `aac-256` …
    label: String,
    bytes: f64,
}

fn blob_of(parts: &js_sys::Array, ctype: &str) -> Result<Blob, String> {
    let bag = web_sys::BlobPropertyBag::new();
    if !ctype.is_empty() { bag.set_type(ctype); }
    Blob::new_with_blob_sequence_and_options(parts, &bag).map_err(|e| store::js_error(&e))
}

async fn fetch_audio(url: &str, on_bytes: &dyn Fn(f64)) -> Result<Fetched, String> {
    let ctl = web_sys::AbortController::new().map_err(|e| store::js_error(&e))?;
    let r = fetch(url, &ctl).await?;
    let header = |k: &str| r.headers().get(k).ok().flatten().unwrap_or_default();
    let (ctype, label) = (header("content-type").split(';').next().unwrap_or("").trim().to_string(), header("x-encore-stream"));
    let Some(body) = r.body() else {
        // no streamed body (an old browser): the whole answer at once
        let p = r.blob().map_err(|e| store::js_error(&e))?;
        let b: Blob = match timed(p, STALL_MS * 10).await {
            Some(Ok(b)) => b.unchecked_into(),
            Some(Err(e)) => return Err(store::js_error(&e)),
            None => { ctl.abort(); return Err("The download stalled".into()); }
        };
        if b.size() <= 0.0 { return Err("The server sent an empty file".into()); }
        on_bytes(b.size());
        return Ok(Fetched { bytes: b.size(), blob: b, ctype, label });
    };
    let reader = web_sys::ReadableStreamDefaultReader::new(&body).map_err(|e| store::js_error(&e))?;
    let parts = js_sys::Array::new();
    let mut chunks = js_sys::Array::new();
    let (mut pending, mut total) = (0.0, 0.0);
    loop {
        let step = match timed(reader.read(), STALL_MS).await {
            Some(Ok(s)) => s,
            Some(Err(e)) => return Err(format!("The download broke off ({})", store::js_error(&e))),
            None => {
                ctl.abort();
                let _ = reader.cancel();
                return Err("The download stalled".into());
            }
        };
        if js_sys::Reflect::get(&step, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true) { break; }
        let chunk: js_sys::Uint8Array = match js_sys::Reflect::get(&step, &"value".into()) { Ok(v) if v.is_object() => v.unchecked_into(), _ => continue };
        let n = chunk.byte_length() as f64;
        chunks.push(&chunk);
        pending += n;
        total += n;
        on_bytes(n);
        if pending >= PART_BYTES {
            let part = Blob::new_with_u8_array_sequence(&chunks).map_err(|e| store::js_error(&e))?;
            parts.push(&part);
            chunks = js_sys::Array::new();
            pending = 0.0;
        }
    }
    if chunks.length() > 0 {
        let part = Blob::new_with_u8_array_sequence(&chunks).map_err(|e| store::js_error(&e))?;
        parts.push(&part);
    }
    if total <= 0.0 { return Err("The server sent an empty file".into()); }
    Ok(Fetched { blob: blob_of(&parts, &ctype)?, ctype, label, bytes: total })
}

/// The album's cover at 600 px, once per album: (saved, saved by this call).
async fn save_cover(cache: &Cache, album_id: &str) -> (bool, bool) {
    let key = store::cover_key(album_id);
    if store::exists(cache, &key).await { return (true, false); }
    let Ok(ctl) = web_sys::AbortController::new() else { return (false, false) };
    let Ok(r) = fetch(&format!("/api/library/albums/{album_id}/cover?s=600"), &ctl).await else { return (false, false) };
    let ctype = r.headers().get("content-type").ok().flatten().unwrap_or_default();
    if !ctype.starts_with("image/") { return (false, false); }
    let Ok(p) = r.blob() else { return (false, false) };
    let Some(Ok(b)) = timed(p, STALL_MS).await else { ctl.abort(); return (false, false) };
    let b: Blob = b.unchecked_into();
    let ok = b.size() > 0.0 && store::write_blob(cache, &key, &b, &ctype).await.is_ok();
    (ok, ok)
}

/// The lyrics, when the server has them (it looks them up when the song has none next to it).
async fn save_lyrics(cache: &Cache, id: i64) -> bool {
    let Ok(ctl) = web_sys::AbortController::new() else { return false };
    let Ok(r) = fetch(&format!("/api/library/tracks/{id}/lyrics"), &ctl).await else { return false };
    let Ok(p) = r.text() else { return false };
    let Some(Ok(t)) = timed(p, STALL_MS).await else { ctl.abort(); return false };
    let Some(text) = t.as_string() else { return false };
    let has_words = serde_json::from_str::<Value>(&text).ok().is_some_and(|v| v.st("lyrics").is_some_and(|l| !l.trim().is_empty()));
    has_words && store::write_text(cache, &store::lyrics_key(id), &text, "application/json").await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_quality_label_reads_like_the_player() {
        assert_eq!(quality_label("flac", true, 16, 44100.0, 900.0), "FLAC 16/44.1");
        assert_eq!(quality_label("flac", false, 24, 96000.0, 0.0), "FLAC 24/96", "FLAC is lossless even when info doesn't say");
        assert_eq!(quality_label("alac", true, 0, 48000.0, 0.0), "ALAC 48 kHz");
        assert_eq!(quality_label("wav", true, 24, 0.0, 0.0), "WAV 24-bit");
        assert_eq!(quality_label("flac", true, 0, 0.0, 0.0), "FLAC");
        assert_eq!(quality_label("mp3", false, 0, 44100.0, 320.0), "MP3 320k");
        assert_eq!(quality_label("aac", false, 0, 44100.0, 255.6), "AAC 256k");
        assert_eq!(quality_label("ogg vorbis", false, 0, 44100.0, 192.0), "Vorbis 192k");
        assert_eq!(quality_label("opus", false, 0, 48000.0, 0.0), "Opus");
        assert_eq!(converted_label("aac", 256), "AAC 256k (converted)");
        assert_eq!(converted_label("opus", 256), "Opus 256k (converted)");
        assert!(store::is_converted(&converted_label("aac", CONVERTED_KBPS)));
    }

    #[test]
    fn what_was_kept_follows_the_stream() {
        let row = json!({ "id": 9, "format": "mp3", "bitrate": 320 });
        let info = json!({ "codec": "flac", "bit_depth": 16, "sample_rate": 44100, "lossless": true, "bitrate": 880,
            "stream": { "kind": "original" }, "offline_ok": true });
        assert_eq!(Kept::of(None, "original", "flac", &info, &row),
            Kept { ext: "flac".into(), format: "flac".into(), quality: "FLAC 16/44.1".into() });
        // the original, with no /info details: the row's bitrate
        assert_eq!(Kept::of(None, "", "mp3", &json!({}), &row), Kept { ext: "mp3".into(), format: "mp3".into(), quality: "MP3 320k".into() });
        assert_eq!(Kept::of(None, "original", "ogg", &json!({ "codec": "ogg vorbis", "bitrate": 160 }), &row).format, "vorbis");
        assert_eq!(Kept::of(None, "original", "m4a", &json!({ "codec": "aac", "bitrate": 256 }), &row),
            Kept { ext: "m4a".into(), format: "aac".into(), quality: "AAC 256k".into() });
        // the lossy conversion asked for
        assert_eq!(Kept::of(Some("aac"), "aac-256", "ape", &info, &row),
            Kept { ext: "aac".into(), format: "aac".into(), quality: "AAC 256k (converted)".into() });
        assert_eq!(Kept::of(Some("opus"), "opus-256", "wv", &info, &row).ext, "opus");
        // the original was asked for but the server converted it
        assert_eq!(Kept::of(None, "flac", "alac", &info, &row).quality, "FLAC (converted)");
    }

    #[test]
    fn only_an_original_the_browser_plays_is_saved_as_it_is() {
        assert!(offers_original(&json!({ "offline_ok": true })));
        assert!(offers_original(&json!({ "stream": { "kind": "original" } })), "a server before offline_ok");
        assert!(!offers_original(&json!({ "offline_ok": false, "stream": { "kind": "flac" } })));
        assert!(!offers_original(&json!({})));
        assert_eq!(stream_url(12, None), "/api/library/tracks/12/stream");
        assert_eq!(stream_url(12, Some("aac")), "/api/library/tracks/12/stream?q=aac&kbps=256");
        assert_eq!(mime_for("mp3"), "audio/mpeg");
        assert_eq!(mime_for("opus"), "audio/ogg");
        assert_eq!(mime_for("xyz"), "application/octet-stream");
    }

    #[test]
    fn a_record_is_built_from_the_row_and_info() {
        let row = json!({ "id": 31, "title": "Song", "artist": "", "albumartist": "Band", "album": "Album", "album_id": "0123456789abcdef",
            "track": 4, "disc": 2, "duration": 201.5, "format": "flac", "artists": [{ "name": "Band" }] });
        let kept = Kept { ext: "flac".into(), format: "flac".into(), quality: "FLAC 16/44.1".into() };
        let r = build_record(&row, &json!({ "duration": 1.0 }), &kept, 25_000_000.0, true, false, 1_760_000_000, "https://music.example.org/");
        assert_eq!(r, Record {
            id: 31, title: "Song".into(), artist: "Band".into(), album: "Album".into(), album_id: "0123456789abcdef".into(),
            ext: "flac".into(), bytes: 25_000_000, quality: "FLAC 16/44.1".into(), cover: true, lyrics: false, saved_at: 1_760_000_000,
            source_server: "https://music.example.org".into(), duration: 201.5, track: 4, disc: 2, format: "flac".into(),
            artists: vec![json!({ "name": "Band" })],
        });
        let bare = build_record(&json!({ "id": 2 }), &json!({ "duration": 99.0 }), &kept, 0.0, false, false, 0, "");
        assert_eq!((bare.duration, bare.bytes, bare.track, bare.artists.len()), (99.0, 0, 0, 0));
    }

    #[test]
    fn only_library_songs_have_an_id() {
        assert_eq!(track_id(&json!({ "id": 5 })), Some(5));
        assert_eq!(track_id(&json!({ "id": 5.0 })), Some(5));
        assert_eq!(track_id(&json!({ "id": "5" })), None, "catalog rows have string ids");
        assert_eq!(track_id(&json!({ "id": 5, "preview": true })), None);
        assert_eq!(track_id(&json!({ "id": 0 })), None);
        assert_eq!(status_words(0), "Not connected to the server");
        assert_eq!(status_words(401), "Not signed in to the server");
        assert_eq!(status_words(404), "The song is not on the server any more");
        assert_eq!(status_words(500), "The server answered 500");
    }
}
