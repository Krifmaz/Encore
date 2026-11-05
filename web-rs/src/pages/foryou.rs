//! Discovery: "For you" (the mixes the server makes from the library and your taste, then ListenBrainz and
//! Last.fm recommendations), genre pages, similar artists and "More like this".
//!
//! The For you page ([`ForYou`]) and a genre's page ([`Genre`]) are Leptos views over signals that fill as their
//! answers come. The rows, tiles and the More like this dialog are shared HTML builders, used by other pages too.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{js_string, json, truthy, J};
use crate::pages::common::{album_tile, bind_tiles, submit_request};
use crate::player;
use crate::ui::{art, empty_state, esc, escv, fmt, icon, icon_attr, modal, status_badge, toast, ModalOpts};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
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

/// The library's mixes as they load: `None` while waiting, then the answer or its error.
type Loading = Option<Result<Value, String>>;

/// What the recommendations part of For you shows.
#[derive(Clone)]
enum Recs {
    /// Still loading.
    Waiting,
    /// GET /api/discover/for-you failed.
    Failed(String),
    /// Neither Last.fm nor ListenBrainz is connected.
    NotConnected,
    /// Connected, nothing yet (the ListenBrainz user's name).
    Nothing(String),
    /// The answer, with each playlist's songs (Last.fm's songs first, as one more list).
    Ready(Value, Vec<Value>, Rc<Vec<Tracks>>),
}

/// The genre chips under "Browse by genre": the library's own (first 15, highlighted), then the top ones.
fn tag_chips(t: &Value) -> Vec<(Value, bool)> {
    let lib: Vec<Value> = t.a("library").into_iter().take(15).collect();
    let mut chips: Vec<(Value, bool)> = lib.iter().map(|g| (g.clone(), true)).collect();
    chips.extend(t.a("top").into_iter().filter(|g| !lib.contains(&Value::String(js_string(g).to_lowercase()))).map(|g| (g, false)));
    chips
}

/// The For you page: the view at once, its parts filled as their answers come (the mixes, the genres, the
/// genre chips and the recommendations).
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let (mixes, genres, tags, recs) = owner.with(|| (RwSignal::new(None), RwSignal::new(Vec::new()), RwSignal::new(Vec::new()), RwSignal::new_local(Recs::Waiting)));
        let again = StoredValue::new_local((view.clone(), ctx));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <ForYou mixes genres tags recs again/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        dom::spawn(async move {
            if let Ok(t) = api::get("/discover/tags").await {
                tags.try_set(tag_chips(&t));
            }
        });
        dom::spawn(async move {
            mixes.try_set(Some(api::get("/taste/mixes?limit=30").await.map_err(|e| e.message)));
            if let Ok(g) = api::get("/taste/genres").await {
                genres.try_set(g.items().into_iter().take(16).collect());
            }
        });
        let answer = match api::get("/discover/for-you").await {
            Err(e) => Recs::Failed(e.message),
            Ok(d) if !d.truthy("connected") => Recs::NotConnected,
            Ok(d) => {
                let lf = d.g("lastfm").clone();
                let mut playlists = d.a("playlists");
                if playlists.is_empty() && !truthy(&lf) {
                    Recs::Nothing(d.s("user"))
                } else {
                    // Last.fm: shown as one more list of songs, plus artists you might like
                    if !lf.a("tracks").is_empty() {
                        playlists.insert(0, json!({ "id": null, "title": "Songs for you", "kind": "lastfm", "tracks": lf.g("tracks").clone() }));
                    }
                    let tracks = Rc::new(playlists.iter().map(|p| shared(p.a("tracks"))).collect());
                    Recs::Ready(d, playlists, tracks)
                }
            }
        };
        recs.try_set(answer);
        Ok(())
    })
}

/// The library's mixes: a tile each, or why there are none, or the error.
fn mixes_view(m: Loading) -> impl IntoView {
    match m {
        None => raw(r#"<div class="skeleton" style="height:180px"></div>"#.to_string()),
        Some(Err(e)) => raw(format!(r#"<div class="error-box">{}</div>"#, esc(&e))),
        Some(Ok(d)) => {
            let mixes = d.a("mixes");
            if mixes.is_empty() {
                return raw(empty_state(&icon("spark"), "No mixes yet", "Mixes appear once the library has songs. They get better the more you play, like and dislike."));
            }
            let note = if d.truthy("cold") { r#"<p class="small muted" style="margin:0 0 10px">Play, like and dislike songs and these mixes learn what you like.</p>"# } else { "" };
            raw(format!(r#"{note}<div class="grid rail">{}</div>"#, mixes.iter().map(mix_tile).collect::<String>()))
        }
    }
}

/// A genre of the library as a chip that plays its mix.
fn genre_chip(x: &Value) -> impl IntoView {
    let cls = if x.f("affinity") > 0.0 { "badge accent" } else { "badge " };
    view! {
        <button class=cls data-mix=format!("genre:{}", x.s("genre")) style="padding:6px 12px;font-size:13px;cursor:pointer" title=format!("Play a {} mix", x.s("title"))>
            {raw(icon_attr("play", r#"width="11" height="11""#))}{x.s("title")}</button>
    }
}

/// One recommended playlist: its title, how much of it is in the library, Play owned, Request missing, its
/// ListenBrainz link and its songs.
fn playlist_view(pi: usize, p: &Value, tracks: Tracks, again: StoredValue<(Element, Ctx), LocalStorage>) -> impl IntoView {
    let missing = tracks.iter().filter(|t| is_missing(t)).count();
    let owned = tracks.iter().filter(|t| in_library(t)).count();
    let counts = (!tracks.is_empty()).then(|| format!(" · {owned}/{} in library", tracks.len()));
    let play = (owned > 0).then(|| {
        let tracks = tracks.clone();
        let go = move |_| player::play_queue(tracks.iter().filter(|t| in_library(t)).map(|t| t.borrow().g("library").clone()).collect(), 0, "list");
        view! { <button class="btn sm" data-play-pl=pi on:click={go} {..html(format!("{}Play owned", icon("play")))}>{""}</button> }
    });
    let request = (missing > 0).then(|| {
        let tracks = tracks.clone();
        let go = move |e: web_sys::MouseEvent| {
            let Some(b) = dom::target_el(&e).and_then(|t| dom::closest_of(&t, "button")) else { return };
            let missing: Vec<Track> = tracks.iter().filter(|t| is_missing(t)).cloned().collect();
            dom::spawn(async move {
                request_all(missing, b).await;
                if let Some((view, ctx)) = again.try_get_value() {
                    let _ = render(view, ctx).await;
                }
            });
        };
        view! { <button class="btn sm primary" data-req-pl=pi on:click={go} {..html(format!("{}Request {missing} missing", icon("download")))}>{""}</button> }
    });
    let link = p.truthy("id").then(|| view! {
        <a class="btn sm ghost" target="_blank" rel="noopener" href=format!("https://listenbrainz.org/playlist/{}", js_string(p.g("id"))) {..html(icon("external"))}>{""}</a>
    });
    let rows: String = tracks.iter().enumerate().map(|(i, t)| track_row(&t.borrow(), i)).collect();
    let table = NodeRef::<leptos::html::Table>::new();
    table.on_load(move |el| bind_tracks(&el, tracks));
    view! {
        <div class="section-head"><div><h2>{p.s("title")}</h2><div class="small muted">{kind(&p.s("kind"))}{counts}</div></div>
            <div class="row">{play}{request}{link}</div></div>
        <div class="card flat"><div class="table-wrap"><table class="table" data-pl=pi node_ref=table><tbody inner_html=rows></tbody></table></div></div>
    }
}

/// The recommendations: errors from the services, artists and albums for you, then each playlist.
fn recs_view(r: Recs, again: StoredValue<(Element, Ctx), LocalStorage>) -> AnyView {
    match r {
        Recs::Waiting => raw(r#"<div class="skeleton" style="height:300px"></div>"#.to_string()).into_any(),
        Recs::Failed(e) => raw(format!(r#"<div class="error-box">{}</div>"#, esc(&e))).into_any(),
        Recs::NotConnected => raw(format!(r#"<div class="card">{}</div>"#, empty_state(&icon("spark"), "Connect Last.fm or ListenBrainz to get recommendations",
            r##"Connect either one on the <a href="#/connections">Connections</a> page. Encore scrobbles what you play here to them, and your other players (Navidrome, Finamp, Plexamp…) can scrobble too."##))).into_any(),
        Recs::Nothing(user) => raw(format!(r#"<div class="card">{}</div>"#, empty_state(&icon("spark"), "No recommendations yet",
            &format!("ListenBrainz builds Weekly Exploration once {} has a few weeks of listens. Check that your players scrobble to ListenBrainz.", esc(&user))))).into_any(),
        Recs::Ready(d, playlists, tracks) => {
            let lf = d.g("lastfm").clone();
            let errors: String = d.a("errors").iter().map(|e| format!(r#"<div class="error-box" style="margin-bottom:12px">{}</div>"#, escv(e))).collect();
            let artists = lf.a("artists");
            let artists = (!artists.is_empty()).then(|| {
                let grid = NodeRef::<leptos::html::Div>::new();
                grid.on_load(|el| bind_tiles(&el));
                let tiles: String = artists.iter().map(artist_card).collect();
                view! {
                    <div class="section-head"><div><h2>"Artists for you"</h2>
                        <div class="small muted">{format!("Similar to what {} plays most on Last.fm", lf.s("user"))}</div></div></div>
                    <div class="grid" id="fy-artists" node_ref=grid inner_html=tiles></div>
                }
            });
            let albums = lf.a("albums");
            let albums = (!albums.is_empty()).then(|| {
                let grid = NodeRef::<leptos::html::Div>::new();
                grid.on_load(|el| bind_tiles(&el));
                let tiles: String = albums.iter().map(|a| album_tile(a, &format!("/search?q={}&type=album", String::from(js_sys::encode_uri_component(&format!("{} {}", a.s("artist"), a.s("title"))))), "", None)).collect();
                view! {
                    <div class="section-head"><div><h2>"Albums for you"</h2><div class="small muted">"The best-known album of artists you might like"</div></div></div>
                    <div class="grid" id="fy-albums" node_ref=grid inner_html=tiles></div>
                }
            });
            let lists = playlists.iter().enumerate().map(|(pi, p)| playlist_view(pi, p, tracks[pi].clone(), again)).collect_view();
            view! { {raw(errors)}{artists}{albums}{lists} }.into_any()
        }
    }
}

/// The For you page: the library's mixes and genres, the recommendations, and the genres to browse.
#[component]
fn ForYou(
    /// The library's mixes.
    mixes: RwSignal<Loading>,
    /// The listener's genres (a mix each).
    genres: RwSignal<Vec<Value>>,
    /// The genres to browse, and whether each is the library's own.
    tags: RwSignal<Vec<(Value, bool)>>,
    /// The recommendations.
    recs: RwSignal<Recs, LocalStorage>,
    /// The page's element and context, to draw it again after Request missing.
    again: StoredValue<(Element, Ctx), LocalStorage>,
) -> impl IntoView {
    // the library's mixes: play one with a click (on its tile or a genre chip)
    let on_lib = |e: web_sys::MouseEvent| {
        if dom::closest(&e, "#fy-gs").is_some() {
            player::genre_shuffle_modal();
            return;
        }
        let Some(m) = dom::closest(&e, "[data-mix]") else { return };
        let id = dom::data(&m, "mix");
        dom::spawn(async move { player::play_mix(&id).await; });
    };
    let chips = move || tags.with(|t| t.iter().map(|(g, mine)| view! {
        <a class=if *mine { "badge accent" } else { "badge " } href=format!("#/tag/{}", String::from(js_sys::encode_uri_component(&js_string(g)))) style="padding:6px 12px;font-size:13px">{js_string(g)}</a>
    }).collect_view());
    view! {
        <h1>"For you"</h1>
        <div id="fy-lib" on:click=on_lib>
            <div class="section-head"><div><h2>"Mixes from your library"</h2><div class="small muted">"Made on this server from what you play, like, skip and dislike"</div></div></div>
            <div id="fy-mixes">{move || mixes_view(mixes.get())}</div>
            <div class="section-head"><div><h2>"Your genres"</h2><div class="small muted">"A mix of each genre in your library"</div></div><button class="btn sm" id="fy-gs" type="button">"Genre shuffle…"</button></div>
            <div id="fy-genres" class="chips">{move || genres.with(|g| g.iter().map(genre_chip).collect_view())}</div>
        </div>
        <div class="section-head"><h2>"From ListenBrainz and Last.fm"</h2></div><p class="muted" style="margin:6px 0 20px">"Recommendations from your ListenBrainz and Last.fm listening. Songs you already have play right here; request the rest in one click."</p>
        <div id="fy-body">{move || recs_view(recs.get(), again)}</div>
        <div class="section-head"><h2>"Browse by genre"</h2></div><div id="fy-tags" class="chips">{chips}</div>
    }
}

/// A genre's page: its name, what it is, and its top artists, 40 at a time with Load more.
pub fn render_tag(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let tag: String = js_sys::decode_uri_component(&ctx.param(0)).map_err(|_| "URI malformed".to_string())?.into();
        // the page asked for (`?page=`), 1 by default
        let page = { let p = ctx.q("page"); if p.is_empty() { 1.0 } else { js_number(&p) } };
        set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let g = owner.with(|| GenreState { page: RwSignal::new(page), summary: RwSignal::new(None), artists: RwSignal::new(None), more: RwSignal::new(false), busy: RwSignal::new(false) });
        let name = tag.clone();
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Genre tag=name.clone() g/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        bind_tiles(&view);
        if let Err(e) = g.load(&tag).await {
            g.artists.try_set(Some(Err(e.message)));
        }
        Ok(())
    })
}

/// A genre page's state.
#[derive(Clone, Copy)]
struct GenreState {
    /// The page of artists last loaded.
    page: RwSignal<f64>,
    /// What the genre is (HTML), once the first page came.
    summary: RwSignal<Option<String>>,
    /// The artists so far (`None`: still loading), or the first page's error.
    artists: RwSignal<Option<Result<Vec<Value>, String>>>,
    /// Whether Load more shows (the last page was full).
    more: RwSignal<bool>,
    /// Load more is loading.
    busy: RwSignal<bool>,
}

impl GenreState {
    /// One page of artists, added to the list; the first also writes the summary.
    async fn load(self, tag: &str) -> Result<(), api::ApiError> {
        let page = self.page.get_untracked();
        let d = api::get(&format!("/discover/tag/{}?page={}", String::from(js_sys::encode_uri_component(tag)), js_num(page))).await?;
        if self.summary.get_untracked().is_none() {
            self.summary.try_set(Some(if d.truthy("summary") {
                format!(r#"{} <span class="faint small">· {}</span>"#, esc(&d.s("summary")), esc(&d.s("source")))
            } else {
                format!(r#"<span class="faint small">Top artists from {}</span>"#, esc(&d.s("source")))
            }));
        }
        let artists = d.a("artists");
        self.more.try_set(artists.len() >= 40);
        self.artists.try_update(|a| match a {
            Some(Ok(list)) => list.extend(artists),
            _ => *a = Some(Ok(artists)),
        });
        Ok(())
    }
}

/// A genre's page.
#[component]
fn Genre(
    /// The genre's name.
    tag: String,
    /// The page's state.
    g: GenreState,
) -> impl IntoView {
    let name = tag.clone();
    let load_more = move |_| {
        let tag = name.clone();
        g.busy.set(true);
        g.page.update(|p| *p += 1.0);
        dom::spawn(async move {
            match g.load(&tag).await {
                Ok(()) => { g.busy.try_set(false); }
                // the button stays disabled, as before
                Err(e) => dom::console_error(&e.message),
            }
        });
    };
    let grid = move || match g.artists.get() {
        None => raw(r#"<div class="skeleton" style="height:200px"></div>"#.to_string()),
        Some(Err(e)) => raw(format!(r#"<div class="error-box">{}</div>"#, esc(&e))),
        Some(Ok(list)) if list.is_empty() && g.page.get_untracked() == 1.0 => raw(empty_state(&icon("users"), "No artists for this genre", "")),
        Some(Ok(list)) => raw(format!(r#"<div class="grid" id="tag-tiles">{}</div>"#, list.iter().map(artist_card).collect::<String>())),
    };
    view! {
        <div class="kicker muted small" style="text-transform:uppercase;letter-spacing:.1em;font-weight:700">"Genre"</div><h1 style="text-transform:capitalize">{tag}</h1>
        <p class="muted" id="tag-sum" style="max-width:80ch" inner_html=move || g.summary.get().unwrap_or_default()></p><div id="tag-grid">{grid}</div>
        <div class="row mt" style="justify-content:center"><button class="btn" class:hidden=move || !g.more.get() id="tag-more" disabled=move || g.busy.get() on:click=load_more>"Load more"</button></div>
    }
}
