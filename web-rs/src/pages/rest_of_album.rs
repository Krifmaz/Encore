//! "Get the rest of the album" (the library album page's ⋯ menu) for every album, whether MusicBrainz lists it
//! or not. `GET /api/discover/album-lookup` says where the album was found:
//!
//! - on MusicBrainz: its catalog page, where the missing songs have Request (as before);
//! - on Deezer, Apple Music or Discogs: that catalogue's track list in a sheet ([`super::discover::ext_sheet`]),
//!   each missing song with Request, and "Request the N missing songs" for all of them at once;
//! - nowhere: a dialog to type the track list in (each missing song then requested), to search the download
//!   sources for the album by name, or – for admins – to pick a download by hand.

use crate::api;
use crate::app;
use crate::dom;
use crate::json::J;
use crate::pages::{common, discover, picker};
use crate::ui::{self, esc, icon, toast, ModalOpts};
use serde_json::{json, Map, Value};
use std::rc::Rc;
use wasm_bindgen::JsCast;

/// `encodeURIComponent(s)`.
fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// The ⋯ menu's "Get the rest of the album" for the library album `album` by `artist` (`rgid` its MusicBrainz
/// release group, "" when it has none).
pub fn get_rest(rgid: &str, artist: &str, album: &str) {
    if !rgid.is_empty() { app::navigate(&format!("/album/{rgid}")); return; }
    let (artist, album) = (artist.to_string(), album.to_string());
    dom::spawn(async move {
        toast("Looking for the album's songs…", "info");
        let r = match api::get(&format!("/discover/album-lookup?artist={}&album={}", enc(&artist), enc(&album))).await {
            Ok(r) => r,
            Err(e) => return toast(&e.message, "warn"),
        };
        if r.truthy("rgid") { return app::navigate(&format!("/album/{}", r.s("rgid"))); }
        match r.s("found").as_str() {
            "elsewhere" => found_elsewhere(&r, &artist),
            _ => not_found(&artist, &album, &r.s("message")),
        }
    });
}

/// The sheet of an album found on another catalogue: its track list with Request on each missing song, and a
/// row on top to request every missing song at once (and, for admins, to pick a download).
fn found_elsewhere(r: &Value, artist: &str) {
    let tracks = r.a("tracks");
    let album_artist = if r.truthy("artist") { r.s("artist") } else { artist.to_string() };
    let missing: Vec<Value> = tracks.iter().filter(|t| !t.truthy("in_library") && !t.truthy("request_status")).cloned().collect();
    let el = discover::ext_sheet(r, tracks, &album_artist);
    let Some(body) = dom::qs_in(&el, ".modal-body") else { return };
    let note = format!("Not on MusicBrainz – track list from {}.", esc(&r.s("source")));
    let req = if missing.is_empty() { String::new() } else {
        format!(r#"<button class="btn primary sm" data-rest-all>{}Request the {} missing song{}</button>"#, icon("plus"), missing.len(), if missing.len() == 1 { "" } else { "s" })
    };
    let pick = if app::is_admin() { format!(r#"<button class="btn sm" data-rest-pick>{}Pick a download</button>"#, icon("search")) } else { String::new() };
    let row = dom::create("div");
    row.set_class_name("row wrap");
    let _ = row.set_attribute("style", "gap:8px;margin-bottom:12px;align-items:center");
    dom::set_html(&row, &format!(r#"<span class="small muted grow">{note}</span>{req}{pick}"#));
    let _ = body.prepend_with_node_1(&row);
    let (r, album_artist) = (r.clone(), album_artist.clone());
    dom::on(&row, "click", move |e| {
        if let Some(b) = dom::closest(e, "[data-rest-all]") {
            let mut o = Map::new();
            o.insert("artist".into(), json!(album_artist));
            o.insert("album".into(), r.g("title").clone());
            o.insert("year".into(), r.g("year").clone());
            o.insert("cover".into(), r.g("cover").clone());
            o.insert("tracks".into(), json!(missing));
            dom::spawn(async move { request_songs(Value::Object(o), Some(b)).await; });
            return;
        }
        if dom::closest(e, "[data-rest-pick]").is_some() {
            picker::picker_modal(json!({ "kind": "album", "artist": album_artist, "title": r.g("title"), "album": r.g("title"),
                "year": r.g("year"), "cover_url": r.g("cover"), "track_count": r.g("tracks").as_array().map(|a| a.len()), "ext": r.g("ext") }));
        }
    });
}

/// No catalogue has the album: type its songs in, search the download sources for it by name, or pick a
/// download by hand (admins).
fn not_found(artist: &str, album: &str, message: &str) {
    let message = if message.is_empty() { format!("No catalogue lists “{album}”.") } else { message.to_string() };
    let pick = if app::is_admin() { format!(r#"<button class="btn" data-rest-pick>{}Pick a download</button>"#, icon("search")) } else { String::new() };
    let body = format!(r#"<p class="muted">{}</p>
      <label class="small muted" for="rest-lines">The album's songs, one per line (numbers and lengths are fine: “1. Song 3:45”)</label>
      <textarea class="input" id="rest-lines" rows="10" placeholder="1. First song&#10;2. Second song"></textarea>
      <label class="switch small" style="margin-top:8px"><input type="checkbox" id="rest-artist-lines"><i></i>Lines are “Artist - Title”</label>"#, esc(&message));
    let foot = format!(r#"<button class="btn" data-rest-name>{}Search downloads for the album</button>{pick}
      <button class="btn primary" data-rest-typed>{}Request the missing songs</button>"#, icon("download"), icon("plus"));
    let md = Rc::new(ui::modal(ModalOpts::new(&esc(&format!("Get the rest of {album}")), &body).footer(&foot)));
    let (artist, album) = (artist.to_string(), album.to_string());
    let m2 = md.clone();
    dom::on(&md.el, "click", move |e| {
        if let Some(b) = dom::closest(e, "[data-rest-typed]") {
            let text = dom::value_of(&m2.el, "#rest-lines");
            if text.trim().is_empty() { return toast("Type the album's songs, one per line", "warn"); }
            let lines = dom::qs_in(&m2.el, "#rest-artist-lines").and_then(|c| c.dyn_into::<web_sys::HtmlInputElement>().ok()).is_some_and(|c| c.checked());
            let body = json!({ "artist": artist, "album": album, "text": text, "artist_lines": lines });
            let m3 = m2.clone();
            dom::spawn(async move { if request_songs(body, Some(b)).await { m3.close(); } });
            return;
        }
        if let Some(b) = dom::closest(e, "[data-rest-name]") {
            // the whole album by name: each download source searches for it, and the songs already here don't stop it
            let body = json!({ "kind": "album", "artist": artist, "title": album, "album": album, "fill_in": true });
            let m3 = m2.clone();
            dom::spawn(async move { if common::submit_request(body, Some(b)).await.is_some() { m3.close(); } });
            return;
        }
        if dom::closest(e, "[data-rest-pick]").is_some() {
            m2.close();
            picker::picker_modal(json!({ "kind": "album", "artist": artist, "title": album, "album": album }));
        }
    });
}

/// `POST /api/discover/album-rest/request` with `body`, its answer as a toast (`btn` disabled meanwhile);
/// true when it went through.
async fn request_songs(body: Value, btn: Option<web_sys::Element>) -> bool {
    if let Some(b) = &btn { dom::set_disabled(b, true); }
    match api::post("/discover/album-rest/request", body).await {
        Ok(r) => {
            let n = r.g("requested").as_i64().unwrap_or(0);
            let already = r.g("already_requested").as_i64().unwrap_or(0);
            let text = if n == 0 { "Every missing song is already requested".to_string() } else {
                format!("Requested {n} song{}{}{}", if n == 1 { "" } else { "s" },
                    if already > 0 { format!(" ({already} already requested)") } else { String::new() },
                    if r.s("status") == "pending" { " – waiting for approval" } else { "" })
            };
            toast(&text, "ok");
            dom::dispatch("encore:requests-changed", None);
            if let Some(b) = &btn { b.remove(); }
            true
        }
        Err(e) => {
            toast(&e.message, "err");
            if let Some(b) = &btn { dom::set_disabled(b, false); }
            false
        }
    }
}
