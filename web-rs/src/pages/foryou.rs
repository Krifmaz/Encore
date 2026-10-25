//! Port of web/js/pages/foryou.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Discovery: "For you" (the mixes the server makes from the library and your taste, then ListenBrainz and
//! Last.fm recommendations), genre pages, similar artists and "More like this".
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{js_string, json, truthy, J};
use crate::pages::common::{album_tile, bind_tiles, submit_request};
use crate::player;
use crate::ui::{art, empty_state, esc, escv, fmt, icon, icon_attr, modal, status_badge, toast, ModalOpts};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsValue;
use web_sys::Element;

/// A track the way the JS held it: one shared object, mutated in place (`t.request_status = …`).
pub(crate) type Track = Rc<RefCell<Value>>;
pub(crate) type Tracks = Rc<Vec<Track>>;
pub(crate) fn shared(tracks: Vec<Value>) -> Tracks { Rc::new(tracks.into_iter().map(|t| Rc::new(RefCell::new(t))).collect()) }
pub(crate) fn is_missing(t: &Track) -> bool { let t = t.borrow(); !t.truthy("library") && !t.truthy("request_status") }
fn in_library(t: &Track) -> bool { t.borrow().truthy("library") }

/// `${n}` of a JS number (2.5 → "2.5", 100 → "100", NaN → "NaN").
fn js_num(n: f64) -> String {
    if n.is_nan() { "NaN".into() } else if n.is_infinite() { (if n > 0.0 { "Infinity" } else { "-Infinity" }).into() } else { js_string(&Value::from(n)) }
}
/// `+s` – JavaScript's `Number(s)` ("" → 0, " 2 " → 2, "abc" → NaN).
#[allow(deprecated)] // `Number::new` is the `Number(value)` call itself; js-sys has no other string-coercing constructor
fn js_number(s: &str) -> f64 { js_sys::Number::new(&JsValue::from_str(s)).value_of() }

/// `KIND[kind] || ''`.
pub(crate) fn kind(k: &str) -> &'static str {
    match k {
        "weekly-exploration" => "New music picked for you every Monday",
        "daily-jams" => "Songs you love, refreshed daily",
        "weekly-jams" => "Your favourites from the past weeks",
        "lastfm" => "Similar to your most played songs on Last.fm",
        _ => "",
    }
}

/// One song row: play it if it's in the library, otherwise request it.
pub(crate) fn track_row(t: &Value, i: usize) -> String {
    let title = t.s("title");
    let artist = t.s("artist");
    let album = t.s("album");
    let play = if t.truthy("library") {
        format!(r#"<button class="btn ghost icon sm" data-play="{i}" aria-label="Play">{}</button>"#, icon_attr("play", r#"width="14""#))
    } else { String::new() };
    let artist_link = if t.truthy("artist_mbid") {
        format!(r##"<a href="#/artist/{}">{}</a>"##, esc(&t.s("artist_mbid")), esc(&crate::ui::tr_inline(t, "artist")))
    } else {
        format!(r##"<a href="#/artist-name/{}">{}</a>"##, String::from(js_sys::encode_uri_component(&artist)), esc(&crate::ui::tr_inline(t, "artist")))
    };
    let status = if t.truthy("library") { r#"<span class="badge ok">In library</span>"#.to_string() }
        else if t.truthy("request_status") { status_badge(&t.s("request_status")) }
        else { format!(r#"<button class="btn sm" data-req="{i}">{}Request</button>"#, icon("plus")) };
    format!(r#"<tr data-i="{i}">
    <td class="num">{play}</td>
    <td><div class="row" style="gap:10px"><div class="thumb">{}</div>
      <div style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div>{}
      <div class="small muted ellipsis">{artist_link}{}</div></div></div></td>
    <td class="muted hide-sm">{}</td>
    <td style="text-align:right;white-space:nowrap">
      <button class="btn sm ghost icon" data-more="{i}" title="More like this" aria-label="More like this">{}</button>
      {status}
    </td></tr>"#,
        art(&t.s("cover"), if t.truthy("album") { &album } else { &title }, ""),
        esc(&crate::ui::tr(t, "title")), crate::ui::tr_sub(t, "title"),
        if t.truthy("album") { format!(" · {}", esc(&crate::ui::tr_inline(t, "album"))) } else { String::new() },
        if t.truthy("length") { fmt::dur(t.f("length")) } else { String::new() },
        icon("spark"))
}

/// `reqBody(t)` – keys the JS left `undefined` are omitted, like JSON.stringify does.
fn req_body(t: &Value) -> Value {
    let or_null = |k: &str| if t.truthy(k) { t.g(k).clone() } else { Value::Null };
    let mut b = serde_json::Map::new();
    b.insert("kind".into(), json!("track"));
    b.insert("mbid".into(), or_null("mbid"));
    if t.has("artist") { b.insert("artist".into(), t.g("artist").clone()); }
    if t.has("title") { b.insert("title".into(), t.g("title").clone()); }
    b.insert("album".into(), or_null("album"));
    if t.has("cover") { b.insert("cover_url".into(), t.g("cover").clone()); }
    if t.has("length") { b.insert("length".into(), t.g("length").clone()); }
    if t.truthy("isrc") { b.insert("isrc".into(), t.g("isrc").clone()); }
    Value::Object(b)
}

fn set_request_status(t: &Track, r: &Value) {
    if let Some(o) = t.borrow_mut().as_object_mut() { o.insert("request_status".into(), r.g("status").clone()); }
}

/// Wire play / request / more-like-this on a table of track rows.
pub(crate) fn bind_tracks(root: &Element, tracks: Tracks) {
    dom::on(root, "click", move |e| {
        if let Some(p) = dom::closest(e, "[data-play]") {
            let Ok(i) = dom::data(&p, "play").parse::<usize>() else { return };
            let Some(t) = tracks.get(i) else { return };
            let playable: Vec<Value> = tracks.iter().filter(|t| in_library(t)).map(|t| t.borrow().g("library").clone()).collect();
            // playable.indexOf(tracks[i].library): its position among the owned songs (-1 → 0 in playQueue)
            let start = if in_library(t) { tracks[..i].iter().filter(|t| in_library(t)).count() } else { 0 };
            return player::play_queue(playable, start, "list");
        }
        if let Some(m) = dom::closest(e, "[data-more]") {
            let Ok(i) = dom::data(&m, "more").parse::<usize>() else { return };
            let Some(t) = tracks.get(i) else { return };
            let t = t.borrow();
            return more_like_this(&t.s("artist"), &t.s("title"), &t.s("mbid"));
        }
        if let Some(r) = dom::closest(e, "[data-req]") {
            let Ok(i) = dom::data(&r, "req").parse::<usize>() else { return };
            let Some(t) = tracks.get(i).cloned() else { return };
            dom::spawn(async move {
                let body = req_body(&t.borrow());
                if let Some(res) = submit_request(body, Some(r)).await { set_request_status(&t, &res); }
            });
        }
    });
}

/// `moreLikeThis(artist, title, mbid)` – the "More like …" modal (mbid "" = none).
pub fn more_like_this(artist: &str, title: &str, mbid: &str) {
    let m = modal(ModalOpts::new(&format!("More like {}", esc(title)), r#"<div class="muted"><span class="spinner"></span> Finding similar songs…</div>"#).wide(true));
    let url = format!("/discover/similar-tracks?artist={}&title={}{}",
        String::from(js_sys::encode_uri_component(artist)), String::from(js_sys::encode_uri_component(title)),
        if !mbid.is_empty() { format!("&mbid={mbid}") } else { String::new() });
    dom::spawn(async move {
        match api::get(&url).await {
            Ok(d) => {
                let Some(body) = m.qs(".modal-body") else { return };
                let tracks = d.a("tracks");
                if tracks.is_empty() {
                    set_html(&body, &empty_state(&icon("spark"), "Nothing similar found", "Add a Last.fm API key in Settings for the best results."));
                    return;
                }
                let tracks = shared(tracks);
                let missing: Vec<Track> = tracks.iter().filter(|t| is_missing(t)).cloned().collect();
                set_html(&body, &format!(r#"<div class="row wrap" style="justify-content:space-between;margin-bottom:10px"><span class="small muted">from {}</span>
        {}</div>
      <div class="table-wrap"><table class="table"><tbody>{}</tbody></table></div>"#,
                    esc(&d.s("source")),
                    if !missing.is_empty() { format!(r#"<button class="btn sm" id="ml-all">{}Request {} missing</button>"#, icon("download"), missing.len()) } else { String::new() },
                    tracks.iter().enumerate().map(|(i, t)| track_row(&t.borrow(), i)).collect::<String>()));
                bind_tracks(&body, tracks);
                if let Some(btn) = dom::qs_in(&body, "#ml-all") {
                    let b = btn.clone();
                    dom::on(&btn, "click", move |_| {
                        let (missing, b) = (missing.clone(), b.clone());
                        dom::spawn(async move { request_all(missing, b).await });
                    });
                }
            }
            Err(e) => {
                if let Some(body) = m.qs(".modal-body") { set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); }
            }
        }
    });
}

/// `requestAll(tracks, btn)`: request every track in turn; stops at the request limit.
pub(crate) async fn request_all(tracks: Vec<Track>, btn: Element) {
    dom::set_disabled(&btn, true);
    let mut ok = 0;
    for t in &tracks {
        let body = req_body(&t.borrow());
        match api::post("/requests", body).await {
            Ok(r) => { set_request_status(t, &r); ok += 1; }
            Err(err) => { if err.message.to_ascii_lowercase().contains("limit") { toast(&err.message, "err"); break; } }
        }
    }
    toast(&format!("{ok} song(s) requested"), "ok");
    dom::dispatch("encore:requests-changed", None);
}

/// Round artist tile for similar artists / genre pages (resolved to MusicBrainz lazily when there is no id).
pub fn artist_card(a: &Value) -> String {
    let name = a.s("name");
    let href = if a.truthy("mbid") { format!("/artist/{}", a.s("mbid")) } else { format!("/artist-name/{}", String::from(js_sys::encode_uri_component(&name))) };
    // (a.name || '?')[0]
    let first = (if a.truthy("name") { name.as_str() } else { "?" }).chars().next().map(|c| c.to_string()).unwrap_or_default();
    format!(r#"<div class="tile round" data-href="{}">
    <div class="art">{}
      {}</div>
    <div class="t ellipsis" style="text-align:center">{}</div>
  </div>"#,
        esc(&href),
        if a.truthy("image") { art(&a.s("image"), &name, "") } else { format!(r#"<div class="ph">{}</div>"#, esc(&first)) },
        if a.truthy("in_library") { r#"<div class="corner" style="top:auto;bottom:8px;left:50%;transform:translateX(-50%)"><span class="badge ok">In library</span></div>"# } else { "" },
        esc(&name))
}

/// Discover before anything is typed: the For you picks as horizontal rows (like Home's Recently added), each with
/// "See all" for the For you page. Owned songs play from their tile; the rest open a search for them.
pub(crate) fn discover_rows(d: &Value) -> String {
    let see_all = r##"<a class="btn sm" href="#/for-you">See all</a>"##;
    let head = |title: &str, sub: &str| format!(r#"<div class="section-head"><div><h2>{}</h2>{}</div>{see_all}</div>"#,
        esc(title), if sub.is_empty() { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(sub)) });
    let search = |q: String, t: &str| format!("/search?q={}&type={t}", String::from(js_sys::encode_uri_component(&q)));
    let lf = d.g("lastfm").clone();
    let mut out = String::new();
    let artists = lf.a("artists");
    if !artists.is_empty() {
        out += &head("Artists for you", "");
        out += &format!(r#"<div class="grid rail">{}</div>"#, artists.iter().map(artist_card).collect::<String>());
    }
    let albums = lf.a("albums");
    if !albums.is_empty() {
        out += &head("Albums for you", "");
        out += &format!(r#"<div class="grid rail">{}</div>"#,
            albums.iter().map(|a| album_tile(a, &search(format!("{} {}", a.s("artist"), a.s("title")), "album"), "", None)).collect::<String>());
    }
    let mut playlists = d.a("playlists");
    if !lf.a("tracks").is_empty() { playlists.insert(0, json!({ "title": "Songs for you", "tracks": lf.g("tracks").clone() })); }
    for p in playlists.iter().filter(|p| !p.a("tracks").is_empty()) {
        let tracks = p.a("tracks");
        let owned = tracks.iter().filter(|t| t.truthy("library")).count();
        out += &head(&p.s("title"), &format!("{owned}/{} in library", tracks.len()));
        out += &format!(r#"<div class="grid rail">{}</div>"#, tracks.iter().map(|t| {
            let tile = json!({ "title": t.s("title"), "artist": t.s("artist"), "cover": t.s("cover"),
                "in_library": t.truthy("library"), "request_status": t.s("request_status") });
            let play = if t.truthy("library") {
                format!(r#"<button class="fab" data-play-song='{}' aria-label="Play">{}</button>"#, esc(&t.g("library").to_string()), icon("play"))
            } else { String::new() };
            album_tile(&tile, &search(format!("{} {}", t.s("artist"), t.s("title")), "track"), &play, Some(&esc(&t.s("artist"))))
        }).collect::<String>());
    }
    out
}

/// One "For you" mix as a tile: a cover from its first album, its name and line, and a play button. A click
/// anywhere on it plays the mix made fresh (`data-mix`).
fn mix_tile(m: &Value) -> String {
    let cover = m.a("covers").first().map(|a| format!("/api/library/albums/{}/cover", js_string(a))).unwrap_or_default();
    format!(r#"<div class="tile" data-mix="{id}" title="Play {title}">
    <div class="art">{art}<div class="overlay"><button class="fab" data-mix="{id}" aria-label="Play">{play}</button></div></div>
    <div class="t ellipsis">{title}</div>
    <div class="s ellipsis">{sub}</div>
  </div>"#,
        id = esc(&m.s("id")), art = art(&cover, &m.s("title"), ""), play = icon("play"), title = esc(&m.s("title")), sub = esc(&m.s("subtitle")))
}

/// The mixes made from the library (`/api/taste/mixes`) and the listener's genres (`/api/taste/genres`) into
/// `#fy-mixes` and `#fy-genres`: each plays on a click.
async fn load_mixes(view: Element) {
    let (Some(rail), Some(chips)) = (dom::qs_in(&view, "#fy-mixes"), dom::qs_in(&view, "#fy-genres")) else { return };
    match api::get("/taste/mixes?limit=30").await {
        Ok(d) => {
            let mixes = d.a("mixes");
            if mixes.is_empty() {
                set_html(&rail, &empty_state(&icon("spark"), "No mixes yet", "Mixes appear once the library has songs. They get better the more you play, like and dislike."));
            } else {
                let note = if d.truthy("cold") { r#"<p class="small muted" style="margin:0 0 10px">Play, like and dislike songs and these mixes learn what you like.</p>"# } else { "" };
                set_html(&rail, &format!(r#"{note}<div class="grid rail">{}</div>"#, mixes.iter().map(mix_tile).collect::<String>()));
            }
        }
        Err(e) => set_html(&rail, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))),
    }
    if let Ok(g) = api::get("/taste/genres").await {
        let list: Vec<Value> = g.items().into_iter().take(16).collect();
        set_html(&chips, &list.iter().map(|x| format!(r#"<button class="badge {}" data-mix="genre:{}" style="padding:6px 12px;font-size:13px;cursor:pointer" title="Play a {} mix">{}{}</button>"#,
            if x.f("affinity") > 0.0 { "accent" } else { "" }, esc(&x.s("genre")), esc(&x.s("title")), icon_attr("play", r#"width="11" height="11""#), esc(&x.s("title")))).collect::<String>());
    }
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, r#"<h1>For you</h1><div id="fy-lib">
    <div class="section-head"><div><h2>Mixes from your library</h2><div class="small muted">Made on this server from what you play, like, skip and dislike</div></div></div>
    <div id="fy-mixes"><div class="skeleton" style="height:180px"></div></div>
    <div class="section-head"><div><h2>Your genres</h2><div class="small muted">A mix of each genre in your library</div></div></div><div id="fy-genres" class="chips"></div></div>
    <div class="section-head"><h2>From ListenBrainz and Last.fm</h2></div><p class="muted" style="margin:6px 0 20px">Recommendations from your ListenBrainz and Last.fm listening. Songs you already have play right here; request the rest in one click.</p>
    <div id="fy-body"><div class="skeleton" style="height:300px"></div></div>
    <div class="section-head"><h2>Browse by genre</h2></div><div id="fy-tags" class="chips"></div>"#);
        {
            // api.get('/discover/tags').then(…).catch(() => {})
            let view = view.clone();
            dom::spawn(async move {
                let Ok(t) = api::get("/discover/tags").await else { return };
                let Some(el) = dom::qs_in(&view, "#fy-tags") else { return };
                let lib: Vec<Value> = t.a("library").into_iter().take(15).collect();
                let mut chips: Vec<(Value, bool)> = lib.iter().map(|g| (g.clone(), true)).collect();
                chips.extend(t.a("top").into_iter().filter(|g| !lib.contains(&Value::String(js_string(g).to_lowercase()))).map(|g| (g, false)));
                set_html(&el, &chips.iter().map(|(g, mine)| format!(r##"<a class="badge {}" href="#/tag/{}" style="padding:6px 12px;font-size:13px">{}</a>"##,
                    if *mine { "accent" } else { "" }, String::from(js_sys::encode_uri_component(&js_string(g))), escv(g))).collect::<String>());
            });
        }
        // the library's mixes: play one with a click (on its tile or a genre chip)
        dom::spawn(load_mixes(view.clone()));
        if let Some(lib) = dom::qs_in(&view, "#fy-lib") { dom::on(&lib, "click", |e| {
            let Some(m) = dom::closest(e, "[data-mix]") else { return };
            let id = dom::data(&m, "mix");
            dom::spawn(async move { player::play_mix(&id).await; });
        }); }
        let Some(body) = dom::qs_in(&view, "#fy-body") else { return Ok(()) };
        let d = match api::get("/discover/for-you").await {
            Ok(d) => d,
            Err(e) => { set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); return Ok(()); }
        };
        if !d.truthy("connected") {
            set_html(&body, &format!(r#"<div class="card">{}</div>"#, empty_state(&icon("spark"), "Connect Last.fm or ListenBrainz to get recommendations",
                r##"Connect either one on the <a href="#/connections">Connections</a> page. Encore scrobbles what you play here to them, and your other players (Navidrome, Finamp, Plexamp…) can scrobble too."##)));
            return Ok(());
        }
        let lf = d.g("lastfm").clone();
        let mut playlists = d.a("playlists");
        if playlists.is_empty() && !truthy(&lf) {
            set_html(&body, &format!(r#"<div class="card">{}</div>"#, empty_state(&icon("spark"), "No recommendations yet",
                &format!("ListenBrainz builds Weekly Exploration once {} has a few weeks of listens. Check that your players scrobble to ListenBrainz.", esc(&d.s("user"))))));
            return Ok(());
        }
        // Last.fm: shown as one more list of songs, plus artists you might like
        if !lf.a("tracks").is_empty() { playlists.insert(0, json!({ "id": null, "title": "Songs for you", "kind": "lastfm", "tracks": lf.g("tracks").clone() })); }
        let lf_artists = lf.a("artists");
        let lf_albums = lf.a("albums");
        let pl_tracks: Rc<Vec<Tracks>> = Rc::new(playlists.iter().map(|p| shared(p.a("tracks"))).collect());
        let errors: String = d.a("errors").iter().map(|e| format!(r#"<div class="error-box" style="margin-bottom:12px">{}</div>"#, escv(e))).collect();
        let artists_html = if !lf_artists.is_empty() {
            format!(r#"<div class="section-head"><div><h2>Artists for you</h2>
      <div class="small muted">Similar to what {} plays most on Last.fm</div></div></div><div class="grid" id="fy-artists">{}</div>"#,
                esc(&lf.s("user")), lf_artists.iter().map(artist_card).collect::<String>())
        } else { String::new() };
        let albums_html = if !lf_albums.is_empty() {
            format!(r#"<div class="section-head"><div><h2>Albums for you</h2><div class="small muted">The best-known album of artists you might like</div></div></div>
      <div class="grid" id="fy-albums">{}</div>"#,
                lf_albums.iter().map(|a| album_tile(a, &format!("/search?q={}&type=album", String::from(js_sys::encode_uri_component(&format!("{} {}", a.s("artist"), a.s("title"))))), "", None)).collect::<String>())
        } else { String::new() };
        let pl_html: String = playlists.iter().enumerate().map(|(pi, p)| {
            let tracks = &pl_tracks[pi];
            let missing = tracks.iter().filter(|t| is_missing(t)).count();
            let owned = tracks.iter().filter(|t| in_library(t)).count();
            format!(r#"<div class="section-head"><div><h2>{}</h2><div class="small muted">{}{}</div></div>
        <div class="row">{}
        {}
        {}</div></div>
      <div class="card flat"><div class="table-wrap"><table class="table" data-pl="{pi}"><tbody>{}</tbody></table></div></div>"#,
                esc(&p.s("title")),
                esc(kind(&p.s("kind"))),
                if !tracks.is_empty() { format!(" · {owned}/{} in library", tracks.len()) } else { String::new() },
                if owned > 0 { format!(r#"<button class="btn sm" data-play-pl="{pi}">{}Play owned</button>"#, icon("play")) } else { String::new() },
                if missing > 0 { format!(r#"<button class="btn sm primary" data-req-pl="{pi}">{}Request {missing} missing</button>"#, icon("download")) } else { String::new() },
                if p.truthy("id") { format!(r#"<a class="btn sm ghost" target="_blank" rel="noopener" href="https://listenbrainz.org/playlist/{}">{}</a>"#, escv(p.g("id")), icon("external")) } else { String::new() },
                tracks.iter().enumerate().map(|(i, t)| track_row(&t.borrow(), i)).collect::<String>())
        }).collect();
        set_html(&body, &format!("{errors}{artists_html}{albums_html}{pl_html}"));
        for tb in dom::qsa_in(&body, "table[data-pl]") {
            let Ok(pi) = dom::data(&tb, "pl").parse::<usize>() else { continue };
            let Some(tracks) = pl_tracks.get(pi) else { continue };
            bind_tracks(&tb, tracks.clone());
        }
        if !lf_artists.is_empty() { if let Some(g) = dom::qs_in(&body, "#fy-artists") { bind_tiles(&g); } }
        if !lf_albums.is_empty() { if let Some(g) = dom::qs_in(&body, "#fy-albums") { bind_tiles(&g); } }
        {
            let (view, ctx, pl_tracks) = (view.clone(), ctx.clone(), pl_tracks.clone());
            dom::on(&body, "click", move |e| {
                if let Some(r) = dom::closest(e, "[data-req-pl]") {
                    let Ok(pi) = dom::data(&r, "reqPl").parse::<usize>() else { return };
                    let Some(tracks) = pl_tracks.get(pi) else { return };
                    let missing: Vec<Track> = tracks.iter().filter(|t| is_missing(t)).cloned().collect();
                    let (view, ctx) = (view.clone(), ctx.clone());
                    dom::spawn(async move { request_all(missing, r).await; let _ = render(view, ctx).await; });
                    return;
                }
                if let Some(p) = dom::closest(e, "[data-play-pl]") {
                    let Ok(pi) = dom::data(&p, "playPl").parse::<usize>() else { return };
                    let Some(tracks) = pl_tracks.get(pi) else { return };
                    player::play_queue(tracks.iter().filter(|t| in_library(t)).map(|t| t.borrow().g("library").clone()).collect(), 0, "list");
                }
            });
        }
        Ok(())
    })
}

pub fn render_tag(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let tag: String = js_sys::decode_uri_component(&ctx.param(0)).map_err(|_| "URI malformed".to_string())?.into();
        // +(ctx.query.page || 1)
        let page = Rc::new(Cell::new({ let p = ctx.q("page"); if p.is_empty() { 1.0 } else { js_number(&p) } }));
        set_html(&view, &format!(r#"<div class="kicker muted small" style="text-transform:uppercase;letter-spacing:.1em;font-weight:700">Genre</div><h1 style="text-transform:capitalize">{}</h1>
    <p class="muted" id="tag-sum" style="max-width:80ch"></p><div id="tag-grid">{}</div>
    <div class="row mt" style="justify-content:center"><button class="btn hidden" id="tag-more">Load more</button></div>"#,
            esc(&tag), r#"<div class="skeleton" style="height:200px"></div>"#));
        let (Some(grid), Some(more)) = (dom::qs_in(&view, "#tag-grid"), dom::qs_in(&view, "#tag-more")) else { return Ok(()) };
        let first = Rc::new(Cell::new(true));

        // load(): one page of artists; the first call also writes the summary
        async fn load(view: &Element, grid: &Element, more: &Element, tag: &str, page: f64, first: &Cell<bool>) -> Result<(), api::ApiError> {
            let d = api::get(&format!("/discover/tag/{}?page={}", String::from(js_sys::encode_uri_component(tag)), js_num(page))).await?;
            if first.get() {
                set_html(grid, r#"<div class="grid" id="tag-tiles"></div>"#);
                if let Some(sum) = dom::qs_in(view, "#tag-sum") {
                    set_html(&sum, &if d.truthy("summary") {
                        format!(r#"{} <span class="faint small">· {}</span>"#, esc(&d.s("summary")), esc(&d.s("source")))
                    } else {
                        format!(r#"<span class="faint small">Top artists from {}</span>"#, esc(&d.s("source")))
                    });
                }
                first.set(false);
            }
            let artists = d.a("artists");
            if let Some(tiles) = dom::qs_in(view, "#tag-tiles") { let _ = tiles.insert_adjacent_html("beforeend", &artists.iter().map(artist_card).collect::<String>()); }
            dom::toggle_class(more, "hidden", artists.len() < 40);
            if artists.is_empty() && page == 1.0 { set_html(grid, &empty_state(&icon("users"), "No artists for this genre", "")); }
            Ok(())
        }

        bind_tiles(&view);
        {
            let (view, grid, btn, tag, page, first) = (view.clone(), grid.clone(), more.clone(), tag.clone(), page.clone(), first.clone());
            dom::on(&more, "click", move |_| {
                let (view, grid, more, tag, page, first) = (view.clone(), grid.clone(), btn.clone(), tag.clone(), page.clone(), first.clone());
                dom::spawn(async move {
                    dom::set_disabled(&more, true);
                    page.set(page.get() + 1.0);
                    if let Err(e) = load(&view, &grid, &more, &tag, page.get(), &first).await { dom::console_error(&e.message); return; }
                    dom::set_disabled(&more, false);
                });
            });
        }
        if let Err(e) = load(&view, &grid, &more, &tag, page.get(), &first).await {
            set_html(&grid, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message)));
        }
        Ok(())
    })
}
