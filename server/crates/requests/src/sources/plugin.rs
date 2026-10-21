//! Sources of plugin programs (uploaded or installed from a URL, protocol 1 of the plugin SDK): a source id
//! in `source_order` or a picker tab that no builtin answers for is a plugin's (`[[sources]]`, or its own id),
//! searched and downloaded through the registry (encore_plugins::Registry::search / download: in the sandbox,
//! with its settings on stdin). This module builds the requests the plugin reads (the SDK's fields: kind,
//! artist, title, album, query, length, track_count, lossless_only, limit, isrc, payload, key), turns its
//! results into the picker's rows and chooses the best match for the request worker.
use crate::Ctx;
use encore_library::matcher::{compare, describe};
pub use encore_plugins::PluginSource;
use encore_plugins::{Call, Failure};
use serde_json::{json, Map, Value};

/// The installed plugin program source with this id (or picker id), switched on or not.
pub fn find(st: &impl Ctx, source: &str) -> Option<PluginSource> {
    st.plugins().plugin_sources().into_iter().find(|s| s.id == source || s.picker == source)
}

/// What a search asks a plugin: the request's fields as the SDK reads them (empty ones left out).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ask {
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub query: Option<String>,
    pub length: Option<f64>,
    pub track_count: Option<usize>,
    pub lossless_only: bool,
    pub isrc: Option<String>,
    pub limit: usize,
}

impl Ask {
    pub fn json(&self, source: &str) -> Value {
        let mut m = Map::new();
        m.insert("kind".into(), json!(self.kind));
        m.insert("source".into(), json!(source));
        for (k, v) in [("artist", &self.artist), ("title", &self.title), ("album", &self.album)] {
            if !v.trim().is_empty() {
                m.insert(k.into(), json!(v.trim()));
            }
        }
        if let Some(q) = self.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            m.insert("query".into(), json!(q));
        }
        if let Some(l) = self.length.filter(|l| *l > 0.0) {
            m.insert("length".into(), json!(l));
        }
        if let Some(n) = self.track_count.filter(|n| *n > 0) {
            m.insert("track_count".into(), json!(n));
        }
        if let Some(i) = self.isrc.as_deref().filter(|i| !i.is_empty()) {
            m.insert("isrc".into(), json!(i));
        }
        m.insert("lossless_only".into(), json!(self.lossless_only));
        m.insert("limit".into(), json!(self.limit.clamp(1, 200)));
        Value::Object(m)
    }
}

/// A plugin source's search through the registry (its plugin installed and switched on).
pub async fn search(st: &impl Ctx, source: &PluginSource, ask: &Ask, call: Option<Call>) -> Result<Vec<Value>, Failure> {
    if !source.enabled {
        return Err(Failure::unsupported(format!("{} is switched off", source.name)));
    }
    st.plugins().search(&source.plugin, &ask.json(&source.id), call).await
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A result's quality line: its own `quality`, else from format, bit depth, sample rate and bitrate.
fn quality(r: &Value) -> String {
    if let Some(q) = r["quality"].as_str().filter(|q| !q.is_empty()) {
        return q.to_string();
    }
    let fmt = r["format"].as_str().unwrap_or("").to_uppercase();
    let depth = r["bit_depth"].as_i64().unwrap_or(0);
    let rate = r["sample_rate"].as_f64().unwrap_or(0.0);
    let br = r["bitrate"].as_i64().unwrap_or(0);
    match (depth > 0 && rate > 0.0, br > 0) {
        (true, _) => {
            let khz = rate / if rate > 1000.0 { 1000.0 } else { 1.0 };
            let khz = if khz.fract() == 0.0 { format!("{}", khz as i64) } else { format!("{khz:.1}") };
            format!("{fmt} {depth}/{khz}").trim().to_string()
        }
        (false, true) => format!("{fmt} {br}k").trim().to_string(),
        _ => fmt,
    }
}

/// A plugin's results (the SDK's `result()` shape, checked by the registry) as the picker's rows: the plugin's
/// own key, payload and link go in the row's payload, for the download.
pub fn picker_rows(source: &PluginSource, results: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = results
        .iter()
        .map(|r| {
            let subtitle = r["subtitle"].as_str().map(String::from).filter(|s| !s.is_empty()).unwrap_or_else(|| {
                [text(&r["artist"]), text(&r["album"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
            });
            json!({
                "source": source.picker, "key": format!("{}:{}", source.id, text(&r["key"])), "title": r["title"], "subtitle": subtitle,
                "quality": quality(r), "lossless": r["lossless"].as_bool().unwrap_or(source.lossless), "tracks": r["tracks"], "size": r["size"],
                "duration": r["duration"], "info": r["info"].as_str().unwrap_or(""), "cover": r["cover"], "isrc": r["isrc"],
                "score": r["score"].as_f64().unwrap_or(0.0),
                "payload": { "plugin_key": r["key"], "plugin_payload": r["payload"], "url": r["url"], "title": r["title"], "artist": r["artist"], "album": r["album"] },
            })
        })
        .collect();
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// What a download asks the plugin: the request's fields, the result's key and payload (a picker row's
/// payload, or a result as the plugin gave it).
pub fn download_request(source: &str, ask: &Ask, chosen: &Value) -> Value {
    let mut req = ask.json(source);
    let key = if chosen.get("plugin_key").is_some() { &chosen["plugin_key"] } else { &chosen["key"] };
    let payload = if chosen.get("plugin_payload").is_some() { &chosen["plugin_payload"] } else { &chosen["payload"] };
    req["key"] = key.clone();
    req["payload"] = payload.clone();
    if let Some(u) = chosen["url"].as_str() {
        req["url"] = json!(u);
    }
    req
}

/// Whether a result is what the request asked for: a song's title and artist (and length, when both say)
/// match; an album's name and artist are in the result's words.
pub fn matches(ask: &Ask, r: &Value) -> bool {
    let (title, artist) = (text(&r["title"]), text(&r["artist"]));
    if ask.kind == "album" {
        let name = [text(&r["album"]), title.clone()].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
        let words = format!("{name} {artist} {}", text(&r["subtitle"]));
        let album_ok = crate::sources::torznab::album_share(&name, if ask.album.is_empty() { &ask.title } else { &ask.album }) >= 0.8;
        let artist_ok = ask.artist.is_empty() || crate::sources::torznab::share(&words, &ask.artist) >= 0.5;
        return album_ok && artist_ok;
    }
    let want = describe(&ask.title, &ask.artist, ask.length, ask.isrc.as_deref(), None);
    let got = describe(&title, if artist.is_empty() { &ask.artist } else { &artist }, r["duration"].as_f64(), r["isrc"].as_str(), None);
    compare(&want, &got, true).matched
}

/// The best result for the request worker: the matching ones, best score first (the plugin's own order for
/// equal scores).
pub fn best<'a>(ask: &Ask, results: &'a [Value], blocked: impl Fn(&Value) -> bool) -> Option<&'a Value> {
    let mut ok: Vec<&Value> = results.iter().filter(|r| matches(ask, r) && !blocked(r)).collect();
    ok.sort_by(|a, b| b["score"].as_f64().unwrap_or(0.0).partial_cmp(&a["score"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    ok.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> PluginSource {
        PluginSource { id: "tone".into(), picker: "tone".into(), name: "Example tone".into(), plugin: "example-tone".into(), enabled: true, kinds: vec!["track".into()], lossless: true }
    }

    #[test]
    fn requests_carry_the_sdk_fields() {
        let ask = Ask { kind: "track".into(), artist: "A".into(), title: "T".into(), length: Some(180.0), limit: 10, ..Default::default() };
        let v = ask.json("tone");
        assert_eq!(v, json!({ "kind": "track", "source": "tone", "artist": "A", "title": "T", "length": 180.0, "lossless_only": false, "limit": 10 }));
        let row = json!({ "plugin_key": "k1", "plugin_payload": { "n": 1 }, "url": "https://x.example/1" });
        let d = download_request("tone", &ask, &row);
        assert_eq!((d["key"].clone(), d["payload"].clone(), d["url"].clone()), (json!("k1"), json!({ "n": 1 }), json!("https://x.example/1")));
        let raw = json!({ "key": "k2", "payload": { "n": 2 } });
        assert_eq!(download_request("tone", &ask, &raw)["key"], "k2");
    }

    #[test]
    fn results_become_picker_rows_and_the_best_match_is_chosen() {
        let results = vec![
            json!({ "key": "a", "title": "Other Song", "artist": "Tone", "score": 90, "format": "flac", "bit_depth": 24, "sample_rate": 96000, "lossless": true }),
            json!({ "key": "b", "title": "Sine", "artist": "Tone", "score": 70, "format": "flac", "bit_depth": 16, "sample_rate": 44100 }),
            json!({ "key": "c", "title": "Sine", "artist": "Tone", "score": 60, "quality": "MP3 320k", "lossless": false }),
        ];
        let rows = picker_rows(&source(), &results);
        assert_eq!(rows[0]["key"], "tone:a");
        assert_eq!(rows[0]["quality"], "FLAC 24/96");
        assert_eq!(rows[1]["quality"], "FLAC 16/44.1");
        assert_eq!(rows[1]["lossless"], true, "the source's own lossless when a result does not say");
        assert_eq!(rows[2]["quality"], "MP3 320k");
        assert_eq!(rows[2]["payload"]["plugin_key"], "c");
        let ask = Ask { kind: "track".into(), artist: "Tone".into(), title: "Sine".into(), limit: 10, ..Default::default() };
        assert_eq!(best(&ask, &results, |_| false).map(|r| r["key"].clone()), Some(json!("b")), "the best of the matching ones");
        assert_eq!(best(&ask, &results, |r| r["key"] == "b").map(|r| r["key"].clone()), Some(json!("c")), "a reported one is passed over");
        let album = Ask { kind: "album".into(), artist: "Tone".into(), title: "Waves".into(), album: "Waves".into(), limit: 10, ..Default::default() };
        let found = vec![json!({ "key": "x", "title": "Waves (Deluxe)", "artist": "Tone", "score": 50 }), json!({ "key": "y", "title": "Other", "artist": "Tone", "score": 99 })];
        assert_eq!(best(&album, &found, |_| false).map(|r| r["key"].clone()), Some(json!("x")));
    }
}
