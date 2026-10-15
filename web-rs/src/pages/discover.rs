//! Port of web/js/pages/discover.js – see docs/ui-rewrite/CONTRACT.md
//! The search page, the artist page (discography, similar artists, releases outside MusicBrainz, photo/bio and
//! popular tracks), the artist-by-name redirect, the album page and the shared request buttons.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{js_string, J};
use crate::pages::{common, foryou, picker};
use crate::player;
use crate::ui::{self, art, availability, confirm_dialog, empty_state, esc, esck, fmt, icon, icon_attr, skeleton_grid, toast, ModalOpts};
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, HtmlElement, HtmlInputElement};

// ---- small helpers for the JS idioms this page uses ----

/// `encodeURIComponent(s)`.
fn encode(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// `history.replaceState(null, '', url)`.
fn replace_state(url: &str) {
    let _ = dom::window().history().and_then(|h| h.replace_state_with_url(&JsValue::NULL, "", Some(url)));
}

/// `window.dispatchEvent(new HashChangeEvent('hashchange'))`.
fn hashchange() {
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") { let _ = dom::window().dispatch_event(&ev); }
}

/// `view.querySelector(sel)` that must exist (we just rendered it).
fn need(view: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(view, sel).ok_or_else(|| format!("{sel} not found")) }

/// `[a, b, c].filter(Boolean).join(sep)` over already-stringified parts ("" is the falsy one).
fn join_truthy(parts: &[String], sep: &str) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(sep)
}

/// `obj.a || obj.b || …` as a string ("" when none is truthy).
fn first_truthy(v: &Value, keys: &[&str]) -> String {
    keys.iter().find(|k| v.truthy(k)).map(|k| v.s(k)).unwrap_or_default()
}

/// `{ dst: src.key }` the JSON.stringify way: a missing property is left out, null stays null.
fn copy(o: &mut Map<String, Value>, dst: &str, src: &Value, key: &str) {
    if src.has(key) { o.insert(dst.to_string(), src.g(key).clone()); }
}
fn put(o: &mut Map<String, Value>, key: &str, v: Value) { o.insert(key.to_string(), v); }

/// `esc(JSON.stringify(v))`.
fn esc_json(v: &Value) -> String { esc(&v.to_string()) }

/// `+el.dataset.key` used as an array index.
fn idx(el: &Element, key: &str) -> Option<usize> { dom::data(el, key).trim().parse().ok() }

/// `arr.map(x => [Math.random(), x]).sort((p, q) => p[0] - q[0]).map(p => p[1])`.
fn shuffled(list: Vec<Value>) -> Vec<Value> {
    let mut pairs: Vec<(f64, Value)> = list.into_iter().map(|x| (js_sys::Math::random(), x)).collect();
    pairs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
    pairs.into_iter().map(|p| p.1).collect()
}

/// `names.join(', ').replace(/, ([^,]*)$/, ' and $1')`.
fn and_join(names: &[String]) -> String {
    let s = names.join(", ");
    match s.rfind(", ") {
        Some(p) if !s[p + 2..].contains(',') => format!("{} and {}", &s[..p], &s[p + 2..]),
        _ => s,
    }
}

/// The JS `reqBtn` helper (declared by discover.js but never used there; kept for parity).
#[allow(dead_code)]
fn req_btn(attrs: &str, label: &str) -> String { format!(r#"<button class="btn sm primary" {attrs}>{}{label}</button>"#, icon("plus")) }

// ---- live request labels: requested → searching → downloading → importing → in library ----------------

/// `liveBadge(status, label)`: the badge of a request – the server's label when it sent one (Not found, Discography requested…).
pub fn live_badge(status: &str, label: &str) -> String {
    let (cls, l) = match status {
        "pending" => ("warn", "Awaiting approval"),
        "queued" => ("info", "Requested"),
        "searching" => ("info", "Searching"),
        "monitoring" | "downloading" => ("accent", "Downloading"),
        "importing" => ("accent", "Importing"),
        "completed" => ("ok", "In library"),
        "failed" => ("err", "Failed"),
        "denied" => ("err", "Denied"),
        "cancelled" => ("", "Cancelled"),
        _ => ("info", "Requested"),
    };
    let spin = if ["searching", "downloading", "importing", "monitoring"].contains(&status) { r#"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"# } else { "" };
    format!(r#"<span class="badge {cls}">{spin}{}</span>"#, esc(if label.is_empty() { l } else { label }))
}

/// `nk(s)`: lower case, letters and digits only.
fn nk(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() }

const EXT_ORDER: [&str; 4] = ["deezer", "apple", "bandcamp", "discogs"];

/// `extKey(ext)`.
fn ext_key(ext: &Value) -> String {
    EXT_ORDER.iter().find(|k| { let v = ext.g(k); !v.is_null() && js_string(v) != "" })
        .map(|k| format!("ext:{k}:{}", js_string(ext.g(k)))).unwrap_or_default()
}

/// `albumKey(a)`: the key a release is known by in request events (server: req_events.rs).
pub fn album_key(a: &Value) -> String {
    if a.truthy("ext") {
        let k = ext_key(a.g("ext"));
        if !k.is_empty() { return k; }
    }
    if a.truthy("id") && !a.s("id").starts_with("ext-") { format!("rg:{}", a.s("id")) } else { format!("a:{}|{}", nk(&a.s("artist")), nk(&a.s("title"))) }
}

/// `songKey(t)`.
pub fn song_key(t: &Value) -> String {
    let id = first_truthy(t, &["recording_id", "id"]);
    if !id.is_empty() { format!("rec:{id}") } else { format!("t:{}|{}", nk(&t.s("artist")), nk(&t.s("title"))) }
}

/// `live(key, inner)`: a spot that follows the request's status as it changes.
fn live(key: &str, inner: &str) -> String { format!(r#"<span class="req-live" data-req-key="{}">{inner}</span>"#, esc(key)) }

/// `albumBadge(a)`.
fn album_badge(a: &Value) -> String {
    if a.truthy("in_library") { r#"<span class="badge ok">In library</span>"#.to_string() }
    else if a.truthy("request_status") { live_badge(&a.s("request_status"), &a.s("request_label")) }
    else { String::new() }
}

/// The page's request event stream: closed when the page goes (dropped with the page's listeners).
struct RequestStream(web_sys::EventSource);
impl Drop for RequestStream {
    fn drop(&mut self) { self.0.close(); }
}

/// `watchRequests(view)`: follow request changes while the page is open (GET /api/requests/events) and update the badges in place.
/// `resync` runs when the stream is back after a drop (server restart, network): the events in between were
/// missed, so the page reloads its statuses.
pub fn watch_requests(view: &Element, resync: Option<Rc<dyn Fn()>>) {
    if js_sys::Reflect::get(&js_sys::global(), &"EventSource".into()).map(|v| v.is_undefined()).unwrap_or(true) { return; }
    let Ok(es) = web_sys::EventSource::new("/api/requests/events") else { return };
    let (v, es2) = (view.clone(), es.clone());
    dom::on(&es, "request", move |e| {
        if !v.is_connected() { es2.close(); return; }
        let Some(m) = e.dyn_ref::<web_sys::MessageEvent>() else { return };
        let Some(text) = m.data().as_string() else { return };
        if let Ok(ev) = serde_json::from_str::<Value>(&text) { apply_request_event(&ev); }
    });
    let dropped = Rc::new(Cell::new(false));
    {
        let dropped = dropped.clone();
        dom::on(&es, "error", move |_| dropped.set(true));
    }
    let v = view.clone();
    dom::on(&es, "hello", move |_| {
        if dropped.get() && v.is_connected() {
            dropped.set(false);
            if let Some(f) = &resync { f(); }
        }
    });
    dom::keep_for(view, RequestStream(es));
}

fn apply_request_event(ev: &Value) {
    let badge = live_badge(&ev.s("status"), &ev.s("label"));
    for k in ev.a("keys").iter().map(js_string) {
        for el in dom::qsa("[data-req-key]") { if dom::data(&el, "reqKey") == k { set_html(&el, &badge); } }
        for el in dom::qsa("[data-req-hide]") { if dom::data(&el, "reqHide") == k { el.remove(); } }
    }
}

/// `artistHref(x)`: a catalogue artist's page, by MusicBrainz id, else looked up by name.
fn artist_href(x: &Value) -> String {
    if x.truthy("artist_id") { format!("#/artist/{}", esck(x, "artist_id")) } else { format!("#/artist-name/{}", encode(&x.s("artist"))) }
}

/// `songRequest(t)`: "Song" / "+ Album" request buttons of a catalogue song.
fn song_request(t: &Value) -> String {
    let mut j = Map::new();
    copy(&mut j, "mbid", t, "id");
    copy(&mut j, "artist", t, "artist");
    copy(&mut j, "title", t, "title");
    copy(&mut j, "album", t, "album");
    copy(&mut j, "cover_url", t, "cover");
    copy(&mut j, "year", t, "year");
    copy(&mut j, "length", t, "length");
    copy(&mut j, "isrc", t, "isrc");
    let with_mode = |mode: &str| { let mut m = j.clone(); put(&mut m, "album_mode", Value::String(mode.into())); Value::Object(m) };
    format!(r#"<div class="row req-group" style="gap:6px">
        <button class="btn sm primary" title="Just this song" data-req-track='{}'>{}Song</button>
        <button class="btn sm" title="Request the song and download the whole album it's on, in the best quality available" data-req-track='{}'>{}+ Album</button>
      </div>"#, esc_json(&with_mode("song")), icon("plus"), esc_json(&with_mode("album")), icon("disc"))
}

fn track_row(t: &Value) -> String {
    let label = if t.truthy("album") { t.s("album") } else { t.s("title") };
    let thumb = if t.truthy("library") {
        format!(r#"<button class="thumb play-thumb" data-play-song='{}' title="Play" aria-label="Play">{}<i>{}</i></button>"#, esc_json(t.g("library")), art(&t.s("cover"), &label, ""), icon("play"))
    } else {
        format!(r#"<div class="thumb">{}</div>"#, art(&t.s("cover"), &label, ""))
    };
    let album = if t.truthy("album") {
        let inner = if t.truthy("release_group_id") {
            format!(r##"<a href="#/album/{}">{}</a>"##, esck(t, "release_group_id"), esck(t, "album"))
        } else { esck(t, "album") };
        format!(" · {inner}")
    } else { String::new() };
    let year = if t.truthy("year") { format!(" · {}", esck(t, "year")) } else { String::new() };
    let len = if t.truthy("length") { fmt::dur(t.f("length")) } else { String::new() };
    let right = if t.truthy("in_library") { availability(t) } else {
        live(&song_key(t), &if t.truthy("request_status") { live_badge(&t.s("request_status"), &t.s("request_label")) } else { song_request(t) })
    };
    format!(r##"<div class="list-item">
    {thumb}
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">{}</div>
      <div class="small muted ellipsis"><a href="{}">{}</a>{album}{year}</div>
    </div>
    <span class="small faint hide-sm">{len}</span>
    {right}
  </div>"##, esck(t, "title"), artist_href(t), esck(t, "artist"))
}

/// `librarySongRow(t)`: a library song (the "In your library" row); plays at once.
fn library_song_row(t: &Value) -> String {
    let label = if t.truthy("album") { t.s("album") } else { t.s("title") };
    let album = if t.truthy("album") { format!(" · {}", esck(t, "album")) } else { String::new() };
    let len = if t.truthy("duration") { fmt::dur(t.f("duration")) } else { String::new() };
    format!(r#"<div class="list-item">
    <button class="thumb play-thumb" data-play-song='{}' title="Play" aria-label="Play">{}<i>{}</i></button>
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">{}</div>
      <div class="small muted ellipsis">{}{album}</div>
    </div>
    <span class="small faint hide-sm">{len}</span>
    {}
  </div>"#, esc_json(t), art(&format!("/api/library/albums/{}/cover", t.s("album_id")), &label, ""), icon("play"), esck(t, "title"), esck(t, "artist"),
        availability(&json!({ "in_library": true })))
}

/// `libraryRow(lib)`: what the library already has for the search (GET /api/library/search).
fn library_row(lib: Option<&Value>) -> String {
    let Some(lib) = lib else { return String::new() };
    let (songs, albums) = (lib.a("songs"), lib.a("albums"));
    if songs.is_empty() && albums.is_empty() { return String::new(); }
    let grid = if albums.is_empty() { String::new() } else {
        format!(r#"<div class="grid">{}</div>"#, albums.iter().take(6).map(common::library_tile).collect::<String>())
    };
    let list = if songs.is_empty() { String::new() } else {
        format!(r#"<div class="card flat mt"><div class="list">{}</div></div>"#, songs.iter().take(5).map(library_song_row).collect::<String>())
    };
    format!(r#"<div class="section-head"><h2>In your library</h2></div>
    {grid}
    {list}"#)
}

/// `albumBody(a)`: the request body of a catalogue album (a MusicBrainz release group, or a Deezer / Apple album).
fn album_body(a: &Value) -> Value {
    let mut j = Map::new();
    if a.truthy("ext") {
        copy(&mut j, "artist", a, "artist");
        copy(&mut j, "title", a, "title");
        copy(&mut j, "album", a, "title");
        copy(&mut j, "year", a, "year");
        copy(&mut j, "cover_url", a, "cover");
        copy(&mut j, "ext", a, "ext");
    } else {
        copy(&mut j, "mbid", a, "id");
        copy(&mut j, "artist", a, "artist");
        copy(&mut j, "title", a, "title");
        copy(&mut j, "album", a, "title");
        copy(&mut j, "year", a, "year");
        copy(&mut j, "cover_url", a, "cover");
    }
    Value::Object(j)
}

/// `albumHref(a)`: where a catalogue album opens: its page, or (only on Deezer / Apple) its artist.
fn album_href(a: &Value) -> String {
    if a.truthy("ext") { artist_href(a)[1..].to_string() } else { format!("/album/{}", a.s("id")) }
}

/// `topResult(top)`: the best song, album or artist, big, with Play when it is in the library or Request when not.
fn top_result(top: &Value) -> String {
    if !crate::json::truthy(top) { return String::new(); }
    let x = top.g("item");
    let (kicker, title, cover, meta, href, actions);
    match top.s("kind").as_str() {
        "artist" => {
            kicker = "Artist".to_string();
            title = x.s("name");
            cover = String::new();
            meta = esc(&join_truthy(&[x.s("type"), x.s("country"), x.s("disambiguation")], " · "));
            href = format!("/artist/{}", x.s("id"));
            actions = format!(r##"<a class="btn primary" href="#/artist/{}">{}Open artist</a>{}"##, esck(x, "id"), icon("user"),
                if x.truthy("in_library") { r#"<span class="badge ok">In library</span>"# } else { "" });
        }
        "album" => {
            kicker = if x.truthy("type") { x.s("type") } else { "Album".to_string() };
            title = x.s("title");
            cover = x.s("cover");
            meta = format!(r#"<a href="{}">{}</a>{}"#, artist_href(x), esck(x, "artist"), if x.truthy("year") { format!(" · {}", esck(x, "year")) } else { String::new() });
            href = album_href(x);
            actions = if x.truthy("library_album_id") {
                format!(r#"<button class="btn primary" data-play-album="{}">{}Play</button>"#, esck(x, "library_album_id"), icon("play"))
            } else if x.truthy("in_library") { availability(x) } else {
                live(&album_key(x), &if x.truthy("request_status") { live_badge(&x.s("request_status"), &x.s("request_label")) }
                    else { format!(r#"<button class="btn primary" data-req-album='{}'>{}Request album</button>"#, esc_json(&album_body(x)), icon("plus")) })
            };
        }
        _ => {
            kicker = "Song".to_string();
            title = x.s("title");
            cover = x.s("cover");
            let album = if x.truthy("album") {
                let inner = if x.truthy("release_group_id") {
                    format!(r##"<a href="#/album/{}">{}</a>"##, esck(x, "release_group_id"), esck(x, "album"))
                } else { esck(x, "album") };
                format!(" · on {inner}")
            } else { String::new() };
            meta = format!(r#"<a href="{}">{}</a>{album}{}"#, artist_href(x), esck(x, "artist"), if x.truthy("year") { format!(" · {}", esck(x, "year")) } else { String::new() });
            href = String::new();
            actions = if x.truthy("library") {
                format!(r#"<button class="btn primary" data-play-song='{}'>{}Play</button>"#, esc_json(x.g("library")), icon("play"))
            } else if x.truthy("in_library") { availability(x) } else {
                live(&song_key(x), &if x.truthy("request_status") { live_badge(&x.s("request_status"), &x.s("request_label")) } else { song_request(x) })
            };
        }
    }
    let href_attr = if href.is_empty() { String::new() } else { format!(r#" data-href="{}""#, esc(&href)) };
    format!(r#"<div class="section-head"><h2>Top result</h2></div>
    <div class="detail-head top-result"{href_attr}>
      <div class="cover">{}</div>
      <div class="grow">
        <div class="kicker">{}</div>
        <h1 class="ellipsis">{}</h1>
        <div class="muted ellipsis">{meta}</div>
        <div class="row wrap mt">{actions}</div>
      </div>
    </div>"#, art(&cover, &title, ""), esc(&kicker), esc(&title))
}

fn artist_tile(a: &Value) -> String {
    let name = a.s("name");
    let first = if name.is_empty() { "?".to_string() } else { name.chars().next().map(|c| c.to_string()).unwrap_or_default() };
    let sub = join_truthy(&[a.s("type"), a.s("country"), a.s("disambiguation")], " · ");
    format!(r#"<div class="tile round" data-href="/artist/{}">
    <div class="art"><div class="ph">{}</div>
      {}
    </div>
    <div class="t ellipsis" style="text-align:center">{}</div>
    <div class="s ellipsis" style="text-align:center">{}</div>
  </div>"#, esck(a, "id"), esc(&first),
        if a.truthy("in_library") { r#"<div class="corner" style="top:auto;bottom:8px;left:50%;transform:translateX(-50%)"><span class="badge ok">In library</span></div>"# } else { "" },
        esc(&name), esc(if sub.is_empty() { " " } else { &sub }))
}

fn album_actions(a: &Value) -> String {
    // owned: play it straight from the library
    if a.truthy("library_album_id") {
        return format!(r#"<button class="fab" title="Play" aria-label="Play" data-play-album="{}">{}</button>"#, esck(a, "library_album_id"), icon("play"));
    }
    if a.truthy("in_library") || a.truthy("request_status") { return String::new(); }
    format!(r#"<button class="fab" title="Request album" data-req-hide="{}" data-req-album='{}'>{}</button>"#, esc(&album_key(a)), esc_json(&album_body(a)), icon("plus"))
}

pub fn bind_request_buttons(root: &Element) {
    dom::on(root, "click", |e| {
        let Some(b) = dom::closest(e, "[data-req-album],[data-req-track],[data-req-artist]") else { return };
        dom::stop(e);
        let attr = |k: &str| dom::data_opt(&b, k).filter(|s| !s.is_empty());
        let (kind, raw) = if let Some(r) = attr("reqAlbum") { ("album", r) }
            else if let Some(r) = attr("reqTrack") { ("track", r) }
            else { ("artist", dom::data(&b, "reqArtist")) };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else { return };
        let mut body = Map::new();
        put(&mut body, "kind", Value::String(kind.into()));
        if let Some(o) = parsed.as_object() { for (k, v) in o { body.insert(k.clone(), v.clone()); } }
        dom::spawn(async move { common::submit_request(Value::Object(body), Some(b)).await; });
    });
}

pub fn render_search(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let q = Rc::new(RefCell::new(ctx.q("q")));
        let typ = Rc::new(RefCell::new({ let t = ctx.q("type"); if t.is_empty() { "all".to_string() } else { t } }));
        let tabs: String = [("all", "Top results"), ("artist", "Artists"), ("album", "Albums"), ("track", "Songs")].iter()
            .map(|(k, l)| format!(r#"<button type="button" data-t="{k}">{l}</button>"#)).collect();
        set_html(&view, &format!(r##"
    <div class="row wrap between" style="margin-bottom:18px">
      <h1>Discover</h1>
      <a class="btn sm" href="#/for-you">{}For you</a>
    </div>
    <form class="search" id="s-form" style="max-width:720px">{}<input class="input" name="q" type="search" enterkeyhint="search" value="{}" placeholder="Search MusicBrainz for artists, albums, songs…" autocomplete="off"></form>
    <div class="tabs mt" id="s-tabs">
      {tabs}
    </div>
    <div id="results" class="mt-lg"></div>"##, icon("spark"), icon("search"), esc(&q.borrow())));
        let input = need(&view, "input[name=q]")?;
        {
            let qv = q.borrow();
            if qv.is_empty() || !dom::matches_media("(pointer: coarse)") {
                dom::focus(&input);
                if let Some(i) = input.dyn_ref::<HtmlInputElement>() { let n = qv.encode_utf16().count() as u32; let _ = i.set_selection_range(n, n); }
            }
        }
        let results = need(&view, "#results")?;
        common::bind_tiles(&results);
        bind_request_buttons(&results);
        let seq = Rc::new(Cell::new(0u64));
        // results update in place – the page (and the search box with the phone keyboard) is never rebuilt while typing
        let run: Rc<dyn Fn() -> Pin<Box<dyn Future<Output = ()>>>> = {
            let (view, results, q, typ, seq) = (view.clone(), results.clone(), q.clone(), typ.clone(), seq.clone());
            Rc::new(move || {
                let (view, results, q, typ, seq) = (view.clone(), results.clone(), q.clone(), typ.clone(), seq.clone());
                Box::pin(async move {
                    let t = typ.borrow().clone();
                    let qv = q.borrow().clone();
                    for b in dom::qsa_in(&view, "#s-tabs button") { dom::toggle_class(&b, "active", dom::data(&b, "t") == t); }
                    replace_state(&format!("#/search?q={}&type={}", encode(&qv), t));
                    seq.set(seq.get() + 1);
                    let mine = seq.get();
                    if qv.is_empty() {
                        set_html(&results, &empty_state(&icon("search"), "Search the entire MusicBrainz catalogue", "Everything you find can be requested with one click."));
                        return;
                    }
                    set_html(&results, &skeleton_grid(12));
                    // the library answers at once; the catalogue (MusicBrainz, Deezer, Apple) takes a moment
                    let lib_q = format!("/library/search?q={}&limit=12", encode(&qv));
                    let disc_q = format!("/discover/search?q={}&type={}", encode(&qv), t);
                    // the library row shows as soon as it's there, and stays when the catalogue fails
                    let got_data = Rc::new(Cell::new(false));
                    let (data, lib) = futures::join!(
                        async { let d = api::get(&disc_q).await; got_data.set(true); d },
                        async {
                            if t != "all" { return None; }
                            let l = api::get(&lib_q).await.ok();
                            let row = library_row(l.as_ref());
                            if mine == seq.get() && view.is_connected() && !got_data.get() && !row.is_empty() {
                                set_html(&results, &(row + &skeleton_grid(8)));
                            }
                            l
                        },
                    );
                    let data = match data {
                        Ok(d) => d,
                        Err(e) => {
                            if mine == seq.get() && view.is_connected() {
                                set_html(&results, &(library_row(lib.as_ref()) + &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))));
                            }
                            return;
                        }
                    };
                    if mine != seq.get() || !view.is_connected() { return; }   // a newer search is already running
                    let mut html = library_row(lib.as_ref()) + &top_result(data.g("top"));
                    let tracks = data.a("tracks");
                    if !tracks.is_empty() {
                        html += &format!(r#"<div class="section-head"><h2>Songs</h2></div><div class="card flat"><div class="list">{}</div></div>"#,
                            tracks.iter().map(track_row).collect::<String>());
                    }
                    let albums = data.a("albums");
                    if !albums.is_empty() {
                        html += &format!(r#"<div class="section-head"><h2>Albums</h2></div><div class="grid">{}</div>"#,
                            albums.iter().map(|a| common::album_tile_with(a, &album_href(a), &album_actions(a), None, Some(&album_key(a)), Some(&album_badge(a)))).collect::<String>());
                    }
                    let artists = data.a("artists");
                    if !artists.is_empty() {
                        html += &format!(r#"<div class="section-head"><h2>Artists</h2></div><div class="grid">{}</div>"#,
                            artists.iter().take(if t == "all" { 6 } else { 60 }).map(artist_tile).collect::<String>());
                    }
                    set_html(&results, &if html.is_empty() { empty_state(&icon("search"), &format!("No results for “{}”", esc(&qv)), "") } else { html });
                })
            })
        };
        {
            let run = run.clone();
            watch_requests(&view, Some(Rc::new(move || dom::spawn(run()))));
        }
        {
            let (q, input, run) = (q.clone(), input.clone(), run.clone());
            dom::on_sel(&view, "#s-form", "submit", move |e| {
                dom::prevent(e);
                *q.borrow_mut() = dom::value(&input).trim().to_string();
                if let Some(h) = input.dyn_ref::<HtmlElement>() { let _ = h.blur(); }
                dom::spawn(run());
            });
        }
        {
            let (q, input2, run) = (q.clone(), input.clone(), run.clone());
            let deb = ui::Debounce::new(650, move || {
                let v = dom::value(&input2).trim().to_string();
                let same = v == *q.borrow();
                if v.encode_utf16().count() > 2 && !same { *q.borrow_mut() = v; dom::spawn(run()); }
            });
            dom::on(&input, "input", move |_| deb.run());
        }
        {
            let (q, typ, input, run) = (q.clone(), typ.clone(), input.clone(), run.clone());
            dom::on_sel(&view, "#s-tabs", "click", move |e| {
                if let Some(t) = dom::closest(e, "[data-t]") {
                    *typ.borrow_mut() = dom::data(&t, "t");
                    *q.borrow_mut() = dom::value(&input).trim().to_string();
                    dom::spawn(run());
                }
            });
        }
        run().await;
        Ok(())
    })
}

const ORDER: [&str; 15] = ["Album", "EP", "Single", "Compilation", "Live", "Soundtrack", "Remix", "DJ-mix", "Mixtape/Street", "Demo", "Other", "Broadcast", "Interview", "Spokenword", "Audiobook"];
const LINK_KEYS: [&str; 9] = ["official homepage", "bandcamp", "soundcloud", "youtube", "streaming", "free streaming", "wikidata", "discogs", "last.fm"];

fn req_body(rg: &Value) -> Value {
    let mut o = Map::new();
    put(&mut o, "kind", Value::String("album".into()));
    copy(&mut o, "mbid", rg, "id");
    copy(&mut o, "artist", rg, "artist");
    copy(&mut o, "title", rg, "title");
    copy(&mut o, "album", rg, "title");
    copy(&mut o, "year", rg, "year");
    copy(&mut o, "cover_url", rg, "cover");
    Value::Object(o)
}

/// `libraryTile({ id: l.album_id, album: l.album, albumartist: l.albumartist, year: l.year })`.
fn lib_tile(l: &Value) -> String {
    common::library_tile(&json!({ "id": l.g("album_id").clone(), "album": l.g("album").clone(), "albumartist": l.g("albumartist").clone(), "year": l.g("year").clone() }))
}

pub fn render_artist(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, &format!(r#"<div class="skeleton" style="height:220px"></div>{}"#, skeleton_grid(8)));
        let a = Rc::new(api::get(&format!("/discover/artist/{}", ctx.param(0))).await.map_err(|e| e.message)?);
        let release_groups = a.a("release_groups");
        // groups keep the insertion order of the JS object
        let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
        for rg in &release_groups {
            let sec = rg.a("secondary");
            let key = if !sec.is_empty() { js_string(&sec[0]) } else if rg.truthy("type") { rg.s("type") } else { "Other".to_string() };
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some(g) => g.1.push(rg.clone()),
                None => groups.push((key, vec![rg.clone()])),
            }
        }
        let rank = |k: &str| ORDER.iter().position(|o| *o == k).map(|i| i as i64 + 1).unwrap_or(99);
        let mut keys: Vec<String> = groups.iter().map(|(k, _)| k.clone()).collect();
        keys.sort_by_key(|k| rank(k));
        let owned = release_groups.iter().filter(|r| r.truthy("in_library")).count();
        let first_cover: Option<Value> = release_groups.iter()
            .find(|r| r.g("type").as_str() == Some("Album") && r.a("secondary").is_empty())
            .and_then(|r| if r.has("cover") { Some(r.g("cover").clone()) } else { None });
        let first_cover_s = first_cover.as_ref().map(js_string).unwrap_or_default();
        let links: Vec<(String, String)> = a.o("links").as_object()
            .map(|o| o.iter().filter(|(k, _)| LINK_KEYS.contains(&k.as_str())).take(6).map(|(k, u)| (k.clone(), js_string(u))).collect())
            .unwrap_or_default();
        // library albums MusicBrainz doesn't list (bootlegs, untagged rips…) still show up
        let rg_ids: HashSet<String> = release_groups.iter().map(|r| r.s("id")).collect();
        let titles: HashSet<String> = release_groups.iter().map(|r| r.s("title").to_lowercase()).collect();
        let library_albums = a.a("library_albums");
        let local_only: Vec<&Value> = library_albums.iter()
            .filter(|l| !(l.truthy("rgid") && rg_ids.contains(&l.s("rgid"))) && !titles.contains(&l.s("album").to_lowercase()))
            .collect();
        let filter = Rc::new(RefCell::new(dom::ls_get("encore.artistFilter").filter(|s| !s.is_empty()).unwrap_or_else(|| "all".to_string())));

        let kicker = format!("{}{}{}",
            esc(&if a.truthy("type") { a.s("type") } else { "Artist".to_string() }),
            if a.truthy("country") { format!(" · {}", esck(&a, "country")) } else { String::new() },
            if a.truthy("begin") { format!(" · since {}", esc(&a.s("begin").chars().take(4).collect::<String>())) } else { String::new() });
        let disamb = if a.truthy("disambiguation") { format!(r#"<p class="muted" style="margin:0 0 10px">{}</p>"#, esck(&a, "disambiguation")) } else { String::new() };
        let chips: String = a.a("tags").iter().map(|t| { let t = js_string(t); format!(r##"<a class="badge" href="#/tag/{}">{}</a>"##, encode(&t), esc(&t)) }).collect();
        let play_btns = if a.truthy("library_tracks") {
            format!(r#"<button class="btn primary" id="play-artist">{}Play</button><button class="btn" id="shuffle-artist">Shuffle</button><button class="btn" id="share-artist" title="Share link">{}Share</button>"#, icon("play"), icon("external"))
        } else { String::new() };
        let request = live(&format!("ar:{}", a.s("id")), &if a.truthy("request_status") { live_badge(&a.s("request_status"), &a.s("request_label")) } else {
            format!(r#"<button class="btn" id="req-disco" title="Every album and EP{} you don't have, from MusicBrainz{}">{}Request discography</button>"#,
                if a.truthy("discography_singles") { " and single" } else { "" }, if a.truthy("catalog_merge") { ", Deezer and Apple Music" } else { "" }, icon("download"))
        });
        let tracks_badge = if a.truthy("library_tracks") { format!(r#"<span class="badge">{} tracks</span>"#, a.s("library_tracks")) } else { String::new() };
        let link_btns: String = links.iter().map(|(k, u)| format!(r#"<a class="btn sm ghost" target="_blank" rel="noopener noreferrer" href="{}">{}</a>"#, esc(u), esc(k))).collect();
        let in_library = if !library_albums.is_empty() {
            format!(r#"<div class="section-head"><h2>In your library <span class="faint small">{}</span></h2></div>
      <div class="grid">{}</div>"#, library_albums.len(), library_albums.iter().map(lib_tile).collect::<String>())
        } else { String::new() };
        let disco_tabs: String = [("all", "All"), ("missing", "Missing"), ("owned", "In library")].iter()
            .map(|(k, l)| format!(r#"<button data-f="{k}">{l}</button>"#)).collect();
        let only_local = if !local_only.is_empty() {
            format!(r#"<div class="section-head"><h2>Only in your library <span class="faint small">{}</span></h2></div>
      <p class="small muted" style="margin:-6px 0 12px">Albums MusicBrainz doesn't list for this artist (or not matched yet).</p>
      <div class="grid">{}</div>"#, local_only.len(), local_only.iter().map(|l| lib_tile(l)).collect::<String>())
        } else { String::new() };

        set_html(&view, &format!(r##"
    <div class="detail-head artist">
      <div class="cover" id="artist-photo">{photo}</div>
      <div class="grow">
        <div class="kicker">{kicker}</div>
        <h1>{name}</h1>
        {disamb}
        <div class="chips">{chips}</div>
        <div class="row wrap mt">
          {play_btns}
          {request}
        </div>
        <div class="row wrap mt small">
          <span class="badge {owned_cls}">{owned} / {n_rg} releases in library</span>
          {tracks_badge}
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/artist/{id}">{external}MusicBrainz</a>
          {link_btns}
        </div>
      </div>
    </div>
    <div id="artist-bio"></div>
    <div id="artist-top"></div>
    {in_library}
    <div class="section-head"><h2>Discography <span class="faint small">{n_rg}</span></h2>
      <div class="tabs" id="disco-filter">{disco_tabs}</div>
    </div>
    <div id="disco"></div>
    <div id="ext-releases"></div>
    <div id="similar"></div>
    {only_local}"##,
            photo = art(&first_cover_s, &a.s("name"), ""),
            name = esck(&a, "name"),
            owned_cls = if owned > 0 { "ok" } else { "" },
            n_rg = release_groups.len(),
            id = esck(&a, "id"),
            external = icon("external"),
        ));

        let disco = need(&view, "#disco")?;
        let groups = Rc::new(RefCell::new(groups));
        let keys = Rc::new(keys);
        let n_rg = release_groups.len();
        let paint: Rc<dyn Fn()> = {
            let (view, disco, groups, keys, filter) = (view.clone(), disco.clone(), groups.clone(), keys.clone(), filter.clone());
            Rc::new(move || {
                let f = filter.borrow().clone();
                for b in dom::qsa_in(&view, "#disco-filter button") { dom::toggle_class(&b, "active", dom::data(&b, "f") == f); }
                let keep = |rg: &Value| f == "all" || (if f == "owned" { rg.truthy("in_library") } else { !rg.truthy("in_library") });
                let g = groups.borrow();
                let html: String = keys.iter().map(|k| {
                    let Some((_, all)) = g.iter().find(|(kk, _)| kk == k) else { return String::new() };
                    let list: Vec<&Value> = all.iter().filter(|rg| keep(rg)).collect();
                    if list.is_empty() { return String::new(); }
                    let missing = all.iter().filter(|rg| !rg.truthy("in_library") && !rg.truthy("request_status")).count();
                    let req_missing = if missing > 0 {
                        format!(r#"<button class="btn sm" data-req-missing="{}">{}Request {} missing</button>"#, esc(k), icon("download"), missing)
                    } else { String::new() };
                    let tiles: String = list.iter().map(|rg| {
                        let sub = esc(&join_truthy(&[first_truthy(rg, &["date", "year"]), if rg.a("secondary").is_empty() { String::new() } else { rg.s("type") }], " · "));
                        common::album_tile_with(rg, &format!("/album/{}", rg.s("id")), &album_actions(rg), Some(&sub), Some(&album_key(rg)), Some(&album_badge(rg)))
                    }).collect();
                    format!(r#"<div class="section-head"><h3>{}s <span class="faint small">{}</span></h3>
          {req_missing}</div>
        <div class="grid">{tiles}</div>"#, esc(k), list.len())
                }).collect();
                let empty_title = if n_rg > 0 { if f == "owned" { "Nothing from this artist in the library yet" } else { "You have everything 🎉" } } else { "No releases on MusicBrainz" };
                set_html(&disco, &if html.is_empty() { empty_state(&icon("disc"), empty_title, "") } else { html });
            })
        };
        paint();
        {
            let (filter, paint) = (filter.clone(), paint.clone());
            dom::on_sel(&view, "#disco-filter", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-f]") else { return };
                *filter.borrow_mut() = dom::data(&b, "f");
                dom::ls_set("encore.artistFilter", &filter.borrow());
                paint();
            });
        }
        {
            let (groups, paint) = (groups.clone(), paint.clone());
            dom::on(&disco, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-req-missing]") else { return };
                let key = dom::data(&b, "reqMissing");
                let missing: Vec<(usize, Value)> = {
                    let g = groups.borrow();
                    let Some((_, list)) = g.iter().find(|(k, _)| *k == key) else { return };
                    list.iter().enumerate().filter(|(_, rg)| !rg.truthy("in_library") && !rg.truthy("request_status")).map(|(i, rg)| (i, rg.clone())).collect()
                };
                let (groups, paint) = (groups.clone(), paint.clone());
                dom::spawn(async move {
                    let title = format!("Request {} {}(s)?", missing.len(), key.to_lowercase());
                    let text = missing.iter().map(|(_, rg)| rg.s("title")).collect::<Vec<_>>().join(" · ");
                    if !confirm_dialog(&title, &text, "Request", false).await { return; }
                    dom::set_disabled(&b, true);
                    let mut ok = 0;
                    for (i, rg) in &missing {
                        match api::post("/requests", req_body(rg)).await {
                            Ok(r) => {
                                if let Some(m) = groups.borrow_mut().iter_mut().find(|(k, _)| *k == key).and_then(|(_, l)| l.get_mut(*i)).and_then(|v| v.as_object_mut()) {
                                    m.insert("request_status".to_string(), r.g("status").clone());
                                    m.insert("request_label".to_string(), Value::Null);
                                }
                                ok += 1;
                            }
                            Err(err) => { toast(&err.message, "err"); if err.message.to_lowercase().contains("limit") { break; } }
                        }
                    }
                    toast(&format!("{ok} release(s) requested"), "ok");
                    dom::dispatch("encore:requests-changed", None);
                    paint();
                });
            });
        }
        common::bind_tiles(&view);
        bind_request_buttons(&view);
        watch_requests(&view, Some(Rc::new(app::route)));
        // Request discography: every album and EP (and single, per the server setting) not owned or requested,
        // from MusicBrainz and the Deezer / Apple Music releases it is missing. The server decides which (title
        // and year dedupe, library check); each release it queues arrives as a request event and marks its tile
        let rel: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
        let paint_ext: Rc<RefCell<Rc<dyn Fn()>>> = Rc::new(RefCell::new(Rc::new(|| {})));
        {
            let (a, view2) = (a.clone(), view.clone());
            let first_cover = first_cover.clone();
            dom::on_sel(&view, "#req-disco", "click", move |_| {
                let Some(b) = dom::qs_in(&view2, "#req-disco") else { return };
                let mut body = Map::new();
                put(&mut body, "kind", Value::String("artist".into()));
                copy(&mut body, "mbid", &a, "id");
                copy(&mut body, "artist", &a, "name");
                if let Some(c) = &first_cover { put(&mut body, "cover_url", c.clone()); }
                let (a, view2) = (a.clone(), view2.clone());
                dom::spawn(async move {
                    let Some(r) = common::submit_request(Value::Object(body), Some(b)).await else { return };
                    let pending = r.s("status") == "pending";
                    let key = format!("ar:{}", a.s("id"));
                    if let Some(spot) = dom::qsa_in(&view2, "[data-req-key]").into_iter().find(|el| dom::data(el, "reqKey") == key) {
                        set_html(&spot, &live_badge(&r.s("status"), if pending { "Awaiting approval" } else { "Discography requested" }));
                    }
                });
            });
        }
        let play_all = {
            let a = a.clone();
            move |shuffle: bool| {
                let a = a.clone();
                dom::spawn(async move {
                    let Ok(t) = api::get(&format!("/discover/artist/{}/library-tracks?name={}", a.s("id"), encode(&a.s("name")))).await else { return };
                    let mut t = t.items();
                    if shuffle { t = shuffled(t); }
                    player::play_queue(t, 0, "list");
                });
            }
        };
        { let p = play_all.clone(); dom::on_sel(&view, "#play-artist", "click", move |_| p(false)); }
        { let p = play_all.clone(); dom::on_sel(&view, "#shuffle-artist", "click", move |_| p(true)); }
        { let name = a.s("name"); dom::on_sel(&view, "#share-artist", "click", move |_| dom::spawn(ui::share_artist(name.clone()))); }

        // similar artists (Last.fm → ListenBrainz → Deezer)
        {
            let (view, a) = (view.clone(), a.clone());
            dom::spawn(async move {
                let Ok(x) = api::get(&format!("/discover/artist/{}/similar", a.s("id"))).await else { return };
                let artists = x.a("artists");
                if !view.is_connected() || artists.is_empty() { return; }
                if let Some(s) = dom::qs_in(&view, "#similar") {
                    set_html(&s, &format!(r#"<div class="section-head"><h2>Similar artists <span class="faint small">{}</span></h2></div>
      <div class="grid">{}</div>"#, esck(&x, "source"), artists.iter().map(foryou::artist_card).collect::<String>()));
                }
            });
        }

        // releases on Deezer / Apple Music that MusicBrainz doesn't list
        let ext = need(&view, "#ext-releases")?;
        set_html(&ext, r#"<div class="small muted mt"><span class="spinner"></span> Checking Deezer and Apple Music for releases MusicBrainz is missing…</div>"#);
        {
            let (view, ext, a, rel, paint_ext) = (view.clone(), ext.clone(), a.clone(), rel.clone(), paint_ext.clone());
            dom::spawn(async move {
                let x = match api::get(&format!("/discover/artist/{}/catalog", a.s("id"))).await { Ok(x) => x, Err(_) => { set_html(&ext, ""); return; } };
                if !view.is_connected() { return; }
                let releases = x.a("releases");
                if releases.is_empty() { set_html(&ext, ""); return; }
                let name = a.s("name");
                let ext_body = {
                    let name = name.clone();
                    move |m: &Value| -> Value {
                        let mut o = Map::new();
                        put(&mut o, "kind", Value::String("album".into()));
                        put(&mut o, "artist", Value::String(name.clone()));
                        copy(&mut o, "title", m, "title");
                        copy(&mut o, "album", m, "title");
                        copy(&mut o, "year", m, "year");
                        copy(&mut o, "cover_url", m, "cover");
                        copy(&mut o, "ext", m, "ext");
                        Value::Object(o)
                    }
                };
                *rel.borrow_mut() = releases;
                let paint: Rc<dyn Fn()> = {
                    let (ext, rel) = (ext.clone(), rel.clone());
                    Rc::new(move || {
                        let releases = rel.borrow();
                        let mut names: Vec<String> = Vec::new();
                        for m in releases.iter() { for s in m.a("sources") { let n = s.s("name"); if !names.contains(&n) { names.push(n); } } }
                        let tiles: String = releases.iter().enumerate().map(|(i, m)| {
                            let corner = if m.truthy("in_library") { r#"<span class="badge ok">In library</span>"#.to_string() }
                                else if m.truthy("request_status") { live_badge(&m.s("request_status"), &m.s("request_label")) }
                                else { r#"<span class="badge warn">Not in MusicBrainz</span>"#.to_string() };
                            let overlay = if m.truthy("library_album_id") {
                                format!(r#"<button class="fab" data-play-album="{}" title="Play" aria-label="Play">{}</button>"#, esck(m, "library_album_id"), icon("play"))
                            } else if m.truthy("in_library") || m.truthy("request_status") { String::new() }
                            else { format!(r#"<button class="fab" data-ext-req="{i}" data-req-hide="{}" title="Request">{}</button>"#, esc(&album_key(m)), icon("plus")) };
                            let sub = esc(&join_truthy(&[first_truthy(m, &["date", "year"]), m.s("type"), if m.truthy("tracks") { format!("{} tracks", m.s("tracks")) } else { String::new() }], " · "));
                            let sources: String = m.a("sources").iter().map(|s| format!(r#"<a class="muted" target="_blank" rel="noopener" href="{}">{}</a>"#, esck(s, "url"), esck(s, "name"))).collect();
                            format!(r##"<div class="tile" data-ext-open="{i}">
        <div class="art">{}
          <div class="corner" data-req-key="{}">{corner}</div>
          <div class="overlay">{overlay}</div></div>
        <div class="t ellipsis" title="{title}">{title}</div>
        <div class="s ellipsis">{sub}</div>
        <div class="row small" style="gap:8px;flex-wrap:wrap;margin-top:4px">
          {sources}
          <a target="_blank" rel="noopener" href="{}" title="Import into MusicBrainz with Harmony">Add to MB</a>
          <a href="#" data-ext-pick="{i}" title="Pick the exact download">Pick</a>
        </div></div>"##, art(&m.s("cover"), &m.s("title"), ""), esc(&album_key(m)), esck(m, "harmony"), title = esck(m, "title"))
                        }).collect();
                        set_html(&ext, &format!(r#"<div class="section-head" style="margin-top:28px"><div><h3>Not in MusicBrainz <span class="faint small">{}</span></h3>
        <div class="small muted">Found on {}. Requests download from Soulseek, streamrip, YouTube Music or Bandcamp; files are tagged from the source's data.
        Add them to MusicBrainz with Harmony and Encore re-tags them automatically.</div></div></div>
      <div class="grid">{tiles}</div>"#, releases.len(), and_join(&names)));
                    })
                };
                *paint_ext.borrow_mut() = paint.clone();
                paint();
                dom::on(&ext, "click", move |e| {
                    if let Some(r) = dom::closest(e, "[data-ext-req]") {
                        dom::stop(e);
                        let Some(i) = idx(&r, "extReq") else { return };
                        let Some(body) = rel.borrow().get(i).map(&ext_body) else { return };
                        let (rel, paint) = (rel.clone(), paint.clone());
                        dom::spawn(async move {
                            if let Some(res) = common::submit_request(body, Some(r)).await {
                                if let Some(m) = rel.borrow_mut().get_mut(i).and_then(|v| v.as_object_mut()) {
                                    m.insert("request_status".to_string(), res.g("status").clone());
                                    m.insert("request_label".to_string(), Value::Null);
                                }
                                paint();
                            }
                        });
                        return;
                    }
                    let p = dom::closest(e, "[data-ext-pick]");
                    let open = if p.is_none() && dom::closest(e, "a, button").is_none() { dom::closest(e, "[data-ext-open]") } else { None };
                    if let Some(open) = open {
                        let Some(m) = idx(&open, "extOpen").and_then(|i| rel.borrow().get(i).cloned()) else { return };
                        if m.truthy("library_album_id") { app::navigate(&format!("/library/album/{}", m.s("library_album_id"))); return; }
                        let name = name.clone();
                        dom::spawn(async move {
                            let mut body = m.g("ext").as_object().cloned().unwrap_or_default();
                            put(&mut body, "artist", Value::String(name.clone()));
                            let tracks = match api::post("/discover/catalog/tracks", Value::Object(body)).await {
                                Ok(t) => t,
                                Err(err) => { toast(&err.message, "err"); return; }
                            };
                            if !crate::json::truthy(&tracks) { return; }
                            ext_sheet(&m, tracks.items(), &name);
                        });
                        return;
                    }
                    if let Some(p) = p {
                        dom::prevent(e);
                        let Some(m) = idx(&p, "extPick").and_then(|i| rel.borrow().get(i).cloned()) else { return };
                        let b = ext_body(&m);
                        let mut o = Map::new();
                        put(&mut o, "kind", Value::String("album".into()));
                        put(&mut o, "artist", Value::String(name.clone()));
                        copy(&mut o, "title", &m, "title");
                        copy(&mut o, "album", &m, "title");
                        copy(&mut o, "year", &m, "year");
                        copy(&mut o, "cover_url", &m, "cover");
                        copy(&mut o, "track_count", &m, "tracks");
                        copy(&mut o, "ext", &b, "ext");
                        picker::picker_modal(Value::Object(o));
                    }
                });
            });
        }

        // photo, bio and popular tracks load after the page (Deezer + Wikipedia, best-effort)
        {
            let (view, a) = (view.clone(), a.clone());
            dom::spawn(async move {
                let Ok(x) = api::get(&format!("/discover/artist/{}/extras", a.s("id"))).await else { return };
                if !view.is_connected() { return; }
                if x.truthy("image") {
                    if let Some(p) = dom::qs_in(&view, "#artist-photo") { set_html(&p, &art(&x.s("image"), &a.s("name"), "")); }
                }
                if x.truthy("bio") {
                    if let Some(b) = dom::qs_in(&view, "#artist-bio") {
                        set_html(&b, &format!(r#"<div class="card flat bio"><p class="clamp" id="bio-text">{}</p>
        <div class="row small"><button class="btn sm ghost" id="bio-more">Show more</button>{}</div></div>"#, esck(&x, "bio"),
                            if x.truthy("bio_url") { format!(r#"<a class="muted" target="_blank" rel="noopener" href="{}">Wikipedia</a>"#, esck(&x, "bio_url")) } else { String::new() }));
                    }
                    let v = view.clone();
                    dom::on_sel(&view, "#bio-more", "click", move |e| {
                        let c = dom::qs_in(&v, "#bio-text").and_then(|t| t.class_list().toggle("clamp").ok()).unwrap_or(false);
                        if let Some(t) = dom::target_el(e) { t.set_text_content(Some(if c { "Show more" } else { "Show less" })); }
                    });
                }
                let top = x.a("top");
                if !top.is_empty() {
                    let playable: Vec<Value> = top.iter().filter(|t| t.truthy("library")).map(|t| t.g("library").clone()).collect();
                    let name = a.s("name");
                    let rows: String = top.iter().enumerate().map(|(i, t)| {
                        let play = if t.truthy("library") {
                            format!(r#"<button class="btn ghost icon sm" data-top-play="{i}" aria-label="Play">{}</button>"#, icon_attr("play", r#"width="14""#))
                        } else { String::new() };
                        let version = if t.truthy("version") { format!(r#"<span class="muted">{}</span>"#, esck(t, "version")) } else { String::new() };
                        let right = if t.truthy("library") { r#"<span class="badge ok">In library</span>"#.to_string() } else {
                            let mut j = Map::new();
                            put(&mut j, "artist", Value::String(name.clone()));
                            put(&mut j, "title", Value::String(format!("{}{}", t.s("title"), if t.truthy("version") { format!(" {}", t.s("version")) } else { String::new() })));
                            copy(&mut j, "album", t, "album");
                            copy(&mut j, "cover_url", t, "cover");
                            copy(&mut j, "length", t, "duration");
                            if t.truthy("isrc") { copy(&mut j, "isrc", t, "isrc"); }
                            put(&mut j, "album_mode", Value::String("song".into()));
                            format!(r#"<button class="btn sm" data-req-track='{}'>{}Track</button>"#, esc_json(&Value::Object(j)), icon("plus"))
                        };
                        format!(r#"<tr>
          <td class="num">{play}</td>
          <td><div class="row" style="gap:10px"><div class="thumb">{}</div><div><div style="font-weight:600">{} {version}</div><div class="small muted">{}</div></div></div></td>
          <td class="muted hide-sm">{}</td>
          <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost icon" data-more-top="{i}" title="More like this" aria-label="More like this">{}</button>
            {right}</td>
        </tr>"#, art(&t.s("cover"), &t.s("album"), ""), esck(t, "title"), esck(t, "album"), fmt::dur(t.f("duration")), icon("spark"))
                    }).collect();
                    if let Some(el) = dom::qs_in(&view, "#artist-top") {
                        set_html(&el, &format!(r#"<div class="section-head"><h2>Popular</h2></div>
        <div class="card flat"><div class="table-wrap"><table class="table"><tbody>{rows}</tbody></table></div></div>"#));
                    }
                    let (top, playable) = (Rc::new(top), Rc::new(playable));
                    dom::on_sel(&view, "#artist-top", "click", move |e| {
                        if let Some(ml) = dom::closest(e, "[data-more-top]") {
                            if let Some(t) = idx(&ml, "moreTop").and_then(|i| top.get(i)) { foryou::more_like_this(&name, &t.s("title"), ""); }
                            return;
                        }
                        if let Some(b) = dom::closest(e, "[data-top-play]") {
                            let Some(i) = idx(&b, "topPlay").filter(|i| *i < top.len()) else { return };
                            // playable.indexOf(top[i].library): its position among the playable ones (-1 → 0 in playQueue)
                            let pos = if top[i].truthy("library") { top[..i].iter().filter(|t| t.truthy("library")).count() } else { 0 };
                            player::play_queue((*playable).clone(), pos, "list");
                        }
                    });
                }
            });
        }
        Ok(())
    })
}

/// `extSheet(m, tracks, artist)`: the track list of a release only Deezer / Apple Music (…) know: owned songs play, missing ones can be requested.
fn ext_sheet(m: &Value, tracks: Vec<Value>, artist: &str) {
    let owned: Vec<Value> = tracks.iter().filter(|t| t.truthy("library")).cloned().collect();
    let queue: Rc<Vec<Value>> = Rc::new(owned.iter().map(|t| t.g("library").clone()).collect());
    let req_cell = |t: &Value| -> String {
        if t.truthy("in_library") { return r#"<span class="badge ok">In library</span>"#.to_string(); }
        let inner = if t.truthy("request_status") { live_badge(&t.s("request_status"), &t.s("request_label")) }
            else if m.truthy("request_status") { String::new() }
            else {
                let mut j = Map::new();
                put(&mut j, "artist", Value::String(if t.truthy("artist") { t.s("artist") } else { artist.to_string() }));
                copy(&mut j, "title", t, "title");
                copy(&mut j, "album", m, "title");
                copy(&mut j, "cover_url", m, "cover");
                copy(&mut j, "year", m, "year");
                if t.truthy("length") { copy(&mut j, "length", t, "length"); }
                if t.truthy("isrc") { copy(&mut j, "isrc", t, "isrc"); }
                put(&mut j, "album_mode", Value::String("song".into()));
                format!(r#"<button class="btn sm" data-req-track='{}'>{}Track</button>"#, esc_json(&Value::Object(j)), icon("plus"))
            };
        live(&song_key(t), &inner)
    };
    let body = if !tracks.is_empty() {
        let head = if !owned.is_empty() {
            format!(r#"<div class="row wrap" style="margin-bottom:12px"><button class="btn primary" data-ext-play-all>{}Play</button><button class="btn" data-ext-shuffle>Shuffle</button>
        <span class="small muted">{} of {} songs in your library</span></div>"#, icon("play"), owned.len(), tracks.len())
        } else { String::new() };
        let rows: String = tracks.iter().enumerate().map(|(i, t)| format!(r#"<tr>
        <td class="num">{}</td>
        <td><div class="ellipsis" style="font-weight:600">{}</div>{}</td>
        <td class="muted" style="width:56px">{}</td>
        <td style="text-align:right;white-space:nowrap">{}</td></tr>"#,
            if t.truthy("library") { format!(r#"<button class="btn ghost icon sm" data-ext-play="{i}" title="Play" aria-label="Play">{}</button>"#, icon_attr("play", r#"width="14""#)) }
                else if t.truthy("track") { t.s("track") } else { String::new() },
            esck(t, "title"),
            if t.truthy("artist") { format!(r#"<div class="small muted ellipsis">{}</div>"#, esck(t, "artist")) } else { String::new() },
            fmt::dur(t.f("length")), req_cell(t))).collect();
        format!(r#"{head}
      <div class="table-wrap"><table class="table"><tbody>{rows}</tbody></table></div>"#)
    } else { r#"<p class="muted">No track list available.</p>"#.to_string() };
    let md = ui::modal(ModalOpts::new(&esck(m, "title"), &body));
    bind_request_buttons(&md.el);
    let tracks = Rc::new(tracks);
    let play = move |i: usize, shuffle: bool| {
        let q = if shuffle { shuffled((*queue).clone()) } else { (*queue).clone() };
        player::play_queue(q, i, "album");
    };
    dom::on(&md.el, "click", move |e| {
        if let Some(one) = dom::closest(e, "[data-ext-play]") {
            let Some(i) = idx(&one, "extPlay").filter(|i| *i < tracks.len()) else { return };
            // owned.indexOf(tracks[i]): its position among the owned songs
            play(tracks[..i].iter().filter(|t| t.truthy("library")).count(), false);
            return;
        }
        if dom::closest(e, "[data-ext-play-all]").is_some() { play(0, false); return; }
        if dom::closest(e, "[data-ext-shuffle]").is_some() { play(0, true); }
    });
}

/// Library artist → full artist page (resolved through MusicBrainz), falling back to the library filter.
pub fn render_artist_by_name(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let name = js_sys::decode_uri_component(&ctx.param(0)).map(String::from).map_err(|_| "URI malformed".to_string())?;
        set_html(&view, r#"<div class="skeleton" style="height:220px"></div>"#);
        let mut to = format!("#/library?artist={}", encode(&name));
        if let Ok(r) = api::get(&format!("/discover/artist-lookup?name={}", encode(&name))).await { to = format!("#/artist/{}", r.s("mbid")); }   // else: not on MusicBrainz
        if !view.is_connected() { return Ok(()); }
        replace_state(&to);           // no extra history entry, so Back skips the redirect
        hashchange();
        Ok(())
    })
}

pub fn render_album(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, r#"<div class="skeleton" style="height:220px"></div>"#);
        let a = Rc::new(api::get(&format!("/discover/album/{}", ctx.param(0))).await.map_err(|e| e.message)?);
        let rel: Option<Value> = if a.truthy("release") { Some(a.g("release").clone()) } else { None };
        let tracks: Vec<Value> = rel.as_ref().map(|r| r.a("tracks")).unwrap_or_default();
        let total: f64 = if rel.is_some() { tracks.iter().map(|t| t.f("length")).sum() } else { 0.0 };
        let multi = rel.as_ref().map(|r| r.f("disc_count") > 1.0).unwrap_or(false);

        let secondary = a.a("secondary");
        let kicker = format!("{}{}", esc(&if a.truthy("type") { a.s("type") } else { "Release".to_string() }),
            if !secondary.is_empty() { format!(" · {}", esc(&secondary.iter().map(js_string).collect::<Vec<_>>().join(", "))) } else { String::new() });
        let meta = format!("{}{}{}",
            if a.truthy("year") { format!(" · {}", esck(&a, "year")) } else { String::new() },
            if rel.is_some() { format!(" · {} tracks · {}", tracks.len(), fmt::long_dur(total)) } else { String::new() },
            match &rel { Some(r) if r.truthy("label") => format!(" · {}", esck(r, "label")), _ => String::new() });
        let chips: String = a.a("tags").iter().map(|t| format!(r#"<span class="badge">{}</span>"#, esc(&js_string(t)))).collect();
        // songs you own play right here, in album order, even when only some of the album is in the library
        let owned_n = tracks.iter().filter(|t| t.truthy("library")).count();
        let owned_btns = if owned_n > 0 {
            let shuffle_icon = { let s = icon("shuffle"); if s.is_empty() { icon("play") } else { s } };
            format!(r#"<button class="btn primary" id="play-owned">{}Play</button><button class="btn" id="shuffle-owned">{}Shuffle</button>"#, icon("play"), shuffle_icon)
        } else { String::new() };
        let open_lib = if a.truthy("library_album_id") {
            format!(r##"<a class="btn" href="#/library/album/{}">{}Open in library</a>"##, esck(&a, "library_album_id"), icon("disc"))
        } else { String::new() };
        let request = if a.truthy("in_library") { if a.truthy("library_album_id") { String::new() } else { availability(&a) } } else {
            live(&format!("rg:{}", a.s("id")), &if a.truthy("request_status") { live_badge(&a.s("request_status"), &a.s("request_label")) } else {
                let mut j = Map::new();
                copy(&mut j, "mbid", &a, "id");
                copy(&mut j, "artist", &a, "artist");
                copy(&mut j, "title", &a, "title");
                copy(&mut j, "album", &a, "title");
                copy(&mut j, "year", &a, "year");
                copy(&mut j, "cover_url", &a, "cover");
                format!(r#"<button class="btn primary" data-req-album='{}'>{}Request album</button>"#, esc_json(&Value::Object(j)), icon("plus"))
            })
        };
        let some_owned = if owned_n > 0 && rel.is_some() && owned_n < tracks.len() {
            format!(r#"<span class="small muted">{owned_n} of {} songs in your library</span>"#, tracks.len())
        } else { String::new() };
        let body = if rel.is_some() {
            let rows: String = tracks.iter().map(|t| {
                let num = if t.truthy("library") {
                    format!(r#"<button class="btn ghost icon sm" data-play-row="{}" title="Play" aria-label="Play">{}</button>"#, esck(t, "recording_id"), icon_attr("play", r#"width="14""#))
                } else { format!("{}{}", if multi { format!("{}-", t.s("disc")) } else { String::new() }, t.s("track")) };
                let title = if t.truthy("library") {
                    format!(r##"<a href="#" data-play-row="{}" style="color:var(--text)">{}</a>"##, esck(t, "recording_id"), esck(t, "title"))
                } else { esck(t, "title") };
                let right = if t.truthy("in_library") { r#"<span class="badge ok">In library</span>"#.to_string() } else {
                    live(&song_key(t), &if t.truthy("request_status") { live_badge(&t.s("request_status"), &t.s("request_label")) }
                    else if a.truthy("request_status") { String::new() }
                    else {
                        let mut j = Map::new();
                        copy(&mut j, "mbid", t, "recording_id");
                        copy(&mut j, "artist", t, "artist");
                        copy(&mut j, "title", t, "title");
                        copy(&mut j, "album", &a, "title");
                        copy(&mut j, "cover_url", &a, "cover");
                        copy(&mut j, "year", &a, "year");
                        copy(&mut j, "length", t, "length");
                        if t.truthy("isrc") { copy(&mut j, "isrc", t, "isrc"); }
                        put(&mut j, "album_mode", Value::String("song".into()));
                        format!(r#"<button class="btn sm" data-req-track='{}'>{}Track</button>"#, esc_json(&Value::Object(j)), icon("plus"))
                    })
                };
                format!(r#"<tr>
        <td class="num">{num}</td>
        <td>{title}</td>
        <td class="hide-sm muted">{}</td>
        <td class="muted">{}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost icon" data-more-track="{rid}" title="More like this" aria-label="More like this">{}</button><button class="btn sm ghost icon" data-pick-track="{rid}" title="Pick a download for this song" aria-label="Pick a download">{}</button>
          {right}</td>
      </tr>"#, esck(t, "artist"), fmt::dur(t.f("length")), icon("spark"), icon("search"), rid = esck(t, "recording_id"))
            }).collect();
            format!(r#"<div class="card flat"><div class="table-wrap"><table class="table">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th>Length</th><th></th></tr></thead>
      <tbody>{rows}</tbody></table></div></div>"#)
        } else { empty_state(&icon("disc"), "No track listing available", "") };

        set_html(&view, &format!(r##"
    <div class="detail-head">
      <div class="cover">{cover}</div>
      <div class="grow">
        <div class="kicker">{kicker}</div>
        <h1>{title}</h1>
        <div class="muted"><a href="#/artist/{artist_id}" style="color:var(--text);font-weight:650">{artist}</a>
          {meta}</div>
        <div class="chips mt">{chips}</div>
        <div class="row wrap mt">
          {owned_btns}
          {open_lib}
          {request}
          {some_owned}
          <button class="btn" id="pick-album" title="Search every source and choose the exact download">{search}Pick download</button>
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/release-group/{id}">{external}MusicBrainz</a>
        </div>
      </div>
    </div>
    {body}"##,
            cover = art(&a.s("cover").replacen("front-250", "front-500", 1), &a.s("title"), ""),
            title = esck(&a, "title"),
            artist_id = esck(&a, "artist_id"),
            artist = esck(&a, "artist"),
            search = icon("search"),
            id = esck(&a, "id"),
            external = icon("external"),
        ));
        bind_request_buttons(&view);
        watch_requests(&view, Some(Rc::new(app::route)));
        {
            let (a, n) = (a.clone(), if rel.is_some() { Value::from(tracks.len()) } else { Value::Null });
            dom::on_sel(&view, "#pick-album", "click", move |_| {
                let mut o = Map::new();
                put(&mut o, "kind", Value::String("album".into()));
                copy(&mut o, "mbid", &a, "id");
                copy(&mut o, "artist", &a, "artist");
                copy(&mut o, "artist_mbid", &a, "artist_id");
                copy(&mut o, "title", &a, "title");
                copy(&mut o, "album", &a, "title");
                copy(&mut o, "year", &a, "year");
                copy(&mut o, "cover_url", &a, "cover");
                put(&mut o, "track_count", n.clone());
                // onDone: reload (the JS re-renders the page through a hashchange)
                picker::picker_modal_with(Value::Object(o), Some(Rc::new(|_| reload_page())));
            });
        }
        let owned: Rc<Vec<Value>> = Rc::new(tracks.iter().filter(|t| t.truthy("library")).cloned().collect());
        let queue: Rc<Vec<Value>> = Rc::new(owned.iter().map(|t| t.g("library").clone()).collect());
        let play_owned = {
            let queue = queue.clone();
            move |i: usize, shuffle: bool| {
                if queue.is_empty() { toast("None of these songs are in your library yet", "err"); return; }
                let q = if shuffle { shuffled((*queue).clone()) } else { (*queue).clone() };
                player::play_queue(q, i, "album");
            }
        };
        { let p = play_owned.clone(); dom::on_sel(&view, "#play-owned", "click", move |_| p(0, false)); }
        { let p = play_owned.clone(); dom::on_sel(&view, "#shuffle-owned", "click", move |_| p(0, true)); }
        {
            let (a, tracks, owned, p) = (a.clone(), Rc::new(tracks), owned.clone(), play_owned.clone());
            dom::on(&view, "click", move |e| {
                if let Some(pr) = dom::closest(e, "[data-play-row]") {
                    dom::prevent(e);
                    let id = dom::data(&pr, "playRow");
                    p(owned.iter().position(|t| t.s("recording_id") == id).unwrap_or(0), false);
                    return;
                }
                if let Some(ml) = dom::closest(e, "[data-more-track]") {
                    let id = dom::data(&ml, "moreTrack");
                    if let Some(t) = tracks.iter().find(|x| x.s("recording_id") == id) { foryou::more_like_this(&t.s("artist"), &t.s("title"), &t.s("recording_id")); }
                    return;
                }
                let Some(b) = dom::closest(e, "[data-pick-track]") else { return };
                let id = dom::data(&b, "pickTrack");
                let Some(t) = tracks.iter().find(|x| x.s("recording_id") == id) else { return };
                let mut o = Map::new();
                put(&mut o, "kind", Value::String("track".into()));
                copy(&mut o, "mbid", t, "recording_id");
                copy(&mut o, "artist", t, "artist");
                copy(&mut o, "title", t, "title");
                copy(&mut o, "album", &a, "title");
                copy(&mut o, "year", &a, "year");
                copy(&mut o, "cover_url", &a, "cover");
                copy(&mut o, "length", t, "length");
                picker::picker_modal_with(Value::Object(o), Some(Rc::new(|_| reload_page())));
            });
        }
        Ok(())
    })
}

/// The album page's `reload`: `window.dispatchEvent(new HashChangeEvent('hashchange'))`.
fn reload_page() {
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") {
        let _ = crate::dom::window().dispatch_event(&ev);
    }
}
