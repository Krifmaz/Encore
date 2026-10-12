//! Port of web/js/playback.js – see docs/ui-rewrite/CONTRACT.md
//!
//! Playback preferences (Settings → Playback): stream quality, pre-loading the next song and what happens
//! when the queue ends. Kept on this device, and – on servers that store preferences – on the account too,
//! the same way appearance.rs does. The player reads them through current().

use crate::dom::{self, ls_get, ls_set};
use crate::json::J;
use serde_json::{json, Value};

/// quality: 'auto' – the original file; a smaller AAC stream for the rest of the song (and the next ten
///                    minutes) when the connection can't keep up
///          'original' – always the original file, whatever the connection does
///          'saver'  – always the smaller AAC stream (mobile data, slow links)
/// kbps:    bitrate of the smaller stream – fixed at 96 (not a user choice)
/// preload: buffer the next song while this one plays (off on very slow connections)
/// autoplay: when the queue ends, keep going with a shuffle of the whole library
pub fn defaults() -> Value { json!({ "quality": "auto", "kbps": 96, "preload": true, "autoplay": true }) }
const KEY: &str = "encore.playback";
const QUALITIES: [&str; 3] = ["auto", "original", "saver"];
/// the smaller stream is always 96 kbps; older saved choices (160 / 256) are ignored
const SMALL_KBPS: i64 = 96;

fn clean(p: &Value) -> Value {
    let mut c = defaults();
    if let (Some(dst), Some(src)) = (c.as_object_mut(), p.as_object()) {
        for (k, v) in src { dst.insert(k.clone(), v.clone()); }
    }
    let quality = c.s("quality");
    if !QUALITIES.contains(&quality.as_str()) { c["quality"] = json!("auto"); }
    c["kbps"] = json!(SMALL_KBPS);
    c["preload"] = json!(c.g("preload") != &json!(false));
    c["autoplay"] = json!(c.g("autoplay") != &json!(false));
    c
}

pub fn current() -> Value {
    match ls_get(KEY).filter(|s| !s.is_empty()) {
        Some(stored) => match serde_json::from_str::<Value>(&stored) { Ok(v) => clean(&v), Err(_) => defaults() },
        // before Settings → Playback existed, "Then shuffle library" lived in its own key
        None => clean(&json!({ "autoplay": ls_get("encore.autoplay").as_deref() != Some("off") })),
    }
}

/// Save on this device and – where the server keeps preferences – on the account (follows you everywhere).
pub async fn save(p: &Value, server_prefs: bool) -> Value {
    let c = clean(p);
    ls_set(KEY, &c.to_string());
    ls_set("encore.autoplay", if c.truthy("autoplay") { "on" } else { "off" });
    dom::dispatch("encore:playback-prefs", Some(serde_wasm_bindgen::to_value(&c).unwrap_or(wasm_bindgen::JsValue::NULL)));
    if server_prefs {
        let _ = crate::api::put("/auth/prefs", json!({ "playback": c })).await;
    }
    c
}

/// After sign-in: the account's playback settings win over this device's.
pub fn adopt_account(user: &Value) {
    let a = user.g("prefs").g("playback").clone();
    if a.is_object() {
        dom::spawn(async move { save(&a, false).await; });
    }
}
