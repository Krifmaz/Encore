//! Home: the greeting with search and Paste a link, the reader's stats, groups listening now, Made for you, New
//! releases, a taste of For you, the downloads in progress and Recently added.
//!
//! A Leptos view ([`Home`]) over one signal per section; each section fills in when its answer comes. The tiles
//! shared with other pages (albums, requests) are still drawn as HTML by `common`.
use crate::api::{self, ApiError};
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::pages::common::{bind_request_actions, bind_tiles, library_tile, request_tile};
use crate::pages::foryou;
use crate::player;
use crate::ui::{self, art, empty_state, esc, icon, modal, toast, ModalOpts};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;

/// The page's sections that come on their own after the page is drawn (`None`: not come, or nothing to show).
#[derive(Clone, Copy)]
struct Sections {
    /// The answer of GET /api/stats/me, when it has the library's and the reader's counts.
    stats: RwSignal<Option<Value>>,
    /// The new releases from library artists.
    releases: RwSignal<Option<Vec<Value>>>,
    /// Whether the kinds Settings → Release types skips are hidden behind a "+N hidden" chip.
    hide_skipped: RwSignal<bool>,
    /// ListenBrainz's playlists for the reader: its user name and the playlists.
    made_for_you: RwSignal<Option<(String, Vec<Value>)>>,
    /// The first songs of the first For you playlist.
    for_you: RwSignal<Option<Vec<Value>>>,
    /// The reader's downloads in progress (the row stays hidden while there are none).
    reqs: RwSignal<Vec<Value>>,
    /// Recently added albums (`None` until they have come).
    recent: RwSignal<Option<Vec<Value>>>,
}

/// `loadReqs`: the reader's active downloads into `reqs`.
async fn load_reqs(reqs: RwSignal<Vec<Value>>) -> Result<(), ApiError> {
    let rows = api::get("/requests?status=active&mine=true&limit=12").await?.items();
    reqs.try_set(rows);
    Ok(())
}

/// The text of the toast once a pasted link was requested.
fn link_toast(r: &Value) -> String {
    if r.s("mode") != "matched" {
        return "Download started".to_string();
    }
    format!(
        "{} song(s) requested{}{}",
        r.s("requested"),
        if r.truthy("name") { format!(" from {}", r.s("name")) } else { String::new() },
        if r.truthy("already_requested") { format!(", {} already requested", r.s("already_requested")) } else { String::new() }
    )
}

/// "Good morning" until noon, "Good afternoon" until six, then "Good evening".
fn greeting(hour: u32) -> &'static str {
    if hour < 12 {
        "Good morning"
    } else if hour < 18 {
        "Good afternoon"
    } else {
        "Good evening"
    }
}

/// Home: the view, mounted at once; the recent albums and the downloads in progress are awaited (their failure
/// is the page's error), the other sections fill in as they come.
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let greet = greeting(js_sys::Date::new_0().get_hours());
        let username = ctx.user.s("username");
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let sec = owner.with(|| Sections {
            stats: RwSignal::new(None),
            releases: RwSignal::new(None),
            hide_skipped: RwSignal::new(crate::pages::discover::hide_skipped_pref()),
            made_for_you: RwSignal::new(None),
            for_you: RwSignal::new(None),
            reqs: RwSignal::new(Vec::new()),
            recent: RwSignal::new(None),
        });
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Home greet username sec/> })) {
            dom::keep(m);
        }
        dom::keep(owner);

        // `loadReqs` as the callback the old page passed around (not awaited, so its errors are dropped)
        let reqs = sec.reqs;
        let reload: Rc<dyn Fn()> = Rc::new(move || dom::spawn(async move { let _ = load_reqs(reqs).await; }));
        bind_tiles(&view);
        // group listens on this server (0.31): "Listening together now", refreshed every 20 s
        crate::group::ui::home_row(&view);
        if let Some(el) = dom::qs_in(&view, "#reqs") {
            bind_request_actions(&el, reload.clone());
        }
        if let Some(el) = dom::qs_in(&view, "#new-releases") {
            let hide = sec.hide_skipped;
            crate::pages::discover::bind_skipped_toggle(&el, Rc::new(move |on| hide.set(on)));
        }
        if let Some(el) = dom::qs_in(&view, "#made-for-you") {
            bind_made_for_you(&el, sec.made_for_you);
        }

        // fun stats
        dom::spawn(async move {
            let Ok(s) = api::get("/stats/me").await else { return };
            // the old page threw (and swallowed it) without these objects
            if s.g("library").is_object() && s.g("me").is_object() {
                sec.stats.try_set(Some(s));
            }
        });
        // new releases from library artists
        dom::spawn(async move {
            let Ok(d) = api::get("/discover/new-releases?limit=12").await else { return };
            let releases = d.a("releases");
            if !releases.is_empty() {
                sec.releases.try_set(Some(releases));
            }
        });
        // ListenBrainz's own playlists for you – only once ListenBrainz is connected
        dom::spawn(async move {
            let Ok(d) = api::get("/discover/made-for-you").await else { return };
            let pls = d.a("playlists");
            if d.truthy("connected") && !pls.is_empty() {
                sec.made_for_you.try_set(Some((d.s("user"), pls)));
            }
        });
        // a taste of For you
        dom::spawn(async move {
            let Ok(d) = api::get("/discover/for-you").await else { return };
            let Some(pl) = d.a("playlists").into_iter().next().filter(crate::json::truthy) else { return };
            // the old page threw (and swallowed it) without a tracks array
            if pl.g("tracks").is_array() {
                sec.for_you.try_set(Some(pl.a("tracks").into_iter().take(6).collect()));
            }
        });

        let (recent, reqs) = futures::join!(api::get("/library/recent?limit=12"), load_reqs(sec.reqs));
        let recent = recent.map_err(|e| e.message)?.items();
        reqs.map_err(|e| e.message)?;
        sec.recent.try_set(Some(recent));
        // setInterval(loadReqs, 5000) + ctx.onLeave(clearInterval)
        dom::interval(&view, 5000, move || reload());
        Ok(())
    })
}

/// The home page: the greeting with search and Paste a link, then the stats, groups listening now, Made for
/// you, New releases, For you, the downloads in progress and Recently added.
#[component]
fn Home(
    /// "Good morning" and so on.
    greet: &'static str,
    /// The reader's name.
    username: String,
    /// The sections that fill in as they come.
    sec: Sections,
) -> impl IntoView {
    let on_search = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let Some(f) = dom::target_el(&e) else { return };
        let q = dom::value_of(&f, "[name=q]").trim().to_string();
        if !q.is_empty() {
            app::navigate(&format!("/search?q={}", String::from(js_sys::encode_uri_component(&q))));
        }
    };
    let on_link = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let Some(input) = dom::target_el(&e).and_then(|f| dom::qs_in(&f, "input")) else { return };
        let url = dom::value(&input).trim().to_string();
        if url.is_empty() {
            return;
        }
        dom::set_disabled(&input, true);
        dom::spawn(async move {
            match api::post("/requests/link", json!({ "url": url })).await {
                Ok(r) => {
                    toast(&link_toast(&r), "ok");
                    dom::set_value(&input, "");
                    dom::dispatch("encore:requests-changed", None);
                    let _ = load_reqs(sec.reqs).await;
                }
                Err(err) => toast(&err.message, "err"),
            }
            dom::set_disabled(&input, false);
        });
    };
    let reqs = move || {
        let rows = sec.reqs.get();
        (!rows.is_empty()).then(|| raw(format!(r#"<div class="hscroll">{}</div>"#, rows.iter().map(request_tile).collect::<String>())))
    };
    let recent = move || {
        sec.recent.get().map(|rows| {
            if rows.is_empty() {
                raw(empty_state(&icon("library"), "Your library is empty", "Search above or paste a link to get started."))
            } else {
                raw(format!(r#"<div class="grid">{}</div>"#, rows.iter().map(library_tile).collect::<String>()))
            }
        })
    };
    view! {
        <section class="hero">
            <h1>{format!("{greet}, {username}")}</h1>
            <p class="muted" style="margin:6px 0 0">"Find any artist, album or song – or paste a link from anywhere."</p>
            <form class="search" id="home-search" on:submit=on_search>{raw(icon("search"))}<input class="input" name="q" type="search" enterkeyhint="search" placeholder="Search artists, albums, songs…" autocomplete="off"/></form>
            <form class="search" id="home-link" style="margin-top:10px" on:submit=on_link>{raw(icon("link"))}<input class="input" name="url" type="url" enterkeyhint="go" placeholder="…or paste a song, album or playlist link to download" autocomplete="off"/></form>
        </section>
        <div class="stats mt" id="stats">{move || sec.stats.get().map(|s| stats_view(&s))}</div>
        <div id="groups-now"></div>
        <div id="made-for-you">{move || sec.made_for_you.get().map(|(user, pls)| made_for_you_view(&user, &pls))}</div>
        <div id="new-releases">{move || sec.releases.get().map(|r| releases_view(&r, sec.hide_skipped.get()))}</div>
        <div id="fy-preview">{move || sec.for_you.get().map(for_you_view)}</div>
        <div id="reqs-wrap" class=move || if sec.reqs.with(Vec::is_empty) { "hidden" } else { "" }><div class="section-head"><h2>"Your downloads in progress"</h2><a class="btn sm" href="#/requests">"All requests"</a></div>
            <div id="reqs">{reqs}</div></div>
        <div class="section-head"><h2>"Recently added"</h2><a class="btn sm" href="#/library?tab=albums&sort=added&dir=desc">"Library"</a></div>
        <div id="recent">{recent}</div>
    }
}

/// The fun stats: the library's size and length, the reader's plays and requests, their top artist and a fact.
fn stats_view(s: &Value) -> impl IntoView {
    let (lib, me) = (s.g("library"), s.g("me"));
    let duration = lib.f("duration");
    let listened = if me.truthy("listened") { format!(" · {}", ui::fmt::long_dur(me.f("listened"))) } else { String::new() };
    let things = if me.f("requests") == 1.0 { "thing" } else { "things" };
    let rank = me.truthy("rank").then(|| view! { {format!(" · #{} on the ", me.s("rank"))}<a href="#/leaderboard">"leaderboard"</a> });
    let top = me.truthy("top_artist").then(|| view! { <div class="stat"><b class="ellipsis" style="display:block">{me.s("top_artist")}</b><span>"your most played artist"</span></div> });
    let fun = s.truthy("fun").then(|| {
        let fun = s.g("fun");
        view! { <div class="stat"><b>{fun.s("value")}</b><span>{fun.s("label")}</span></div> }
    });
    view! {
        <div class="stat"><b>{ui::fmt::num(lib.f("tracks"))}</b><span>"songs in the library"</span></div>
        <div class="stat"><b>{ui::fmt::long_dur(duration)}</b><span>{format!("to listen to{}", if duration > 86400.0 { " non-stop" } else { "" })}</span></div>
        <div class="stat"><b>{ui::fmt::num(me.f("plays"))}</b><span>{format!("songs you played{listened}")}</span></div>
        <div class="stat"><b>{ui::fmt::num(me.f("requests"))}</b><span>{format!("{things} you requested")}{rank}</span></div>
        {top}
        {fun}
    }
}

/// New releases from artists in the library; the kinds Settings → Release types skips hide behind a
/// "+N hidden" chip while `hide`.
fn releases_view(releases: &[Value], hide: bool) -> impl IntoView {
    let skipped = releases.iter().filter(|r| r.truthy("skipped")).count();
    let toggle = if skipped > 0 { crate::pages::discover::skipped_toggle(hide, skipped) } else { String::new() };
    let tiles = releases
        .iter()
        .filter(|r| !(hide && r.truthy("skipped")))
        .map(|r| {
            let href = if r.truthy("mbid") { format!("/album/{}", r.s("mbid")) } else { format!("/artist-name/{}", String::from(js_sys::encode_uri_component(&r.s("artist")))) };
            let corner = r.truthy("in_library").then(|| view! { <span class="badge ok">"In library"</span> });
            let kind = if r.truthy("type") && r.s("type") != "Album" { format!(" · {}", r.s("type")) } else { String::new() };
            view! {
                <div class="tile" data-href=href>
                    <div class="art">{raw(art(&r.s("cover"), &r.s("title"), ""))}<div class="corner">{corner}</div></div>
                    <div class="t ellipsis" title=r.s("title")>{r.s("title")}</div>
                    <div class="s ellipsis">{format!("{} · {}{kind}", r.s("artist"), r.s("date"))}</div></div>
            }
        })
        .collect_view();
    view! {
        <div class="section-head"><h2>"New releases"</h2><span class="row wrap" style="gap:10px;justify-content:flex-end"><span class="small muted">"from artists in your library"</span>{raw(toggle)}</span></div>
        <div class="grid">{tiles}</div>
    }
}

/// ListenBrainz's playlists for the reader: a tile each, with how many of its songs are in the library.
fn made_for_you_view(user: &str, pls: &[Value]) -> impl IntoView {
    let tiles = pls
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let covers: Vec<String> = p.a("covers").iter().map(|c| c.as_str().unwrap_or("").to_string()).collect();
            let art_html = if covers.len() >= 4 {
                format!(r#"<div class="collage">{}</div>"#, covers.iter().take(4).map(|c| format!(r#"<div class="cell">{}</div>"#, art(c, &p.s("title"), ""))).collect::<String>())
            } else if let Some(c) = covers.first() {
                art(c, &p.s("title"), "")
            } else {
                format!(r#"<div class="ph">{}</div>"#, icon("spark"))
            };
            let k = foryou::kind(&p.s("kind"));
            let play = p.truthy("owned").then(|| view! { <button class="fab" data-mfy-play=i aria-label="Play" {..html(icon("play"))}>{""}</button> });
            view! {
                <div class="tile" data-mfy=i>
                    <div class="art">{raw(art_html)}
                        <div class="corner"><span class="badge">{format!("{}/{} in library", p.s("owned"), p.s("total"))}</span></div>
                        <div class="overlay">{play}</div></div>
                    <div class="t ellipsis" title=p.s("title")>{p.s("title")}</div>
                    <div class="s ellipsis">{if k.is_empty() { "From ListenBrainz" } else { k }}</div></div>
            }
        })
        .collect_view();
    view! {
        <div class="section-head"><div><h2>"Made for you"</h2><div class="small muted">{format!("ListenBrainz builds these from what {user} listens to")}</div></div>
            <a class="btn sm ghost" target="_blank" rel="noopener" href=format!("https://listenbrainz.org/user/{}/playlists/", String::from(js_sys::encode_uri_component(user))) title="On ListenBrainz" {..html(icon("external"))}>{""}</a></div>
        <div class="grid">{tiles}</div>
    }
}

/// Made for you's clicks: Play on a tile plays its songs in the library; the tile opens the playlist.
fn bind_made_for_you(el: &Element, made_for_you: RwSignal<Option<(String, Vec<Value>)>>) {
    let play_owned = |p: &Value| player::play_queue(p.a("tracks").iter().filter(|t| t.truthy("library")).map(|t| t.g("library").clone()).collect(), 0, "list");
    dom::on(el, "click", move |e| {
        let Some(pls) = made_for_you.try_get_untracked().flatten().map(|(_, p)| p) else { return };
        if let Some(b) = dom::closest(e, "[data-mfy-play]") {
            dom::stop(e);
            if let Some(p) = dom::data(&b, "mfy-play").parse::<usize>().ok().and_then(|i| pls.get(i)) { play_owned(p); }
            return;
        }
        let Some(t) = dom::closest(e, "[data-mfy]") else { return };
        let Some(p) = dom::data(&t, "mfy").parse::<usize>().ok().and_then(|i| pls.get(i)).cloned() else { return };
        let tracks = foryou::shared(p.a("tracks"));
        let missing: Vec<foryou::Track> = tracks.iter().filter(|t| foryou::is_missing(t)).cloned().collect();
        let k = foryou::kind(&p.s("kind"));
        let m = modal(ModalOpts::new(&p.s("title"), &format!(r#"<div class="row wrap" style="justify-content:space-between;margin-bottom:10px"><span class="small muted">{} · {}/{} in library</span>
          <span class="row">{}{}</span></div>
        <div class="table-wrap"><table class="table"><tbody>{}</tbody></table></div>"#,
            esc(k), p.s("owned"), p.s("total"),
            if p.truthy("owned") { format!(r#"<button class="btn sm" id="mfy-play">{}Play owned</button>"#, icon("play")) } else { String::new() },
            if !missing.is_empty() { format!(r#"<button class="btn sm primary" id="mfy-req">{}Request {} missing</button>"#, icon("download"), missing.len()) } else { String::new() },
            tracks.iter().enumerate().map(|(i, t)| foryou::track_row(&t.borrow(), i)).collect::<String>())).wide(true));
        let Some(body) = m.qs(".modal-body") else { return };
        foryou::bind_tracks(&body, tracks);
        if let Some(b) = dom::qs_in(&body, "#mfy-play") {
            dom::on(&b, "click", move |_| play_owned(&p));
        }
        if let Some(b) = dom::qs_in(&body, "#mfy-req") {
            let (missing, btn) = (missing, b.clone());
            dom::on(&b, "click", move |e| {
                let (missing, btn) = (missing.clone(), dom::target_el(e).and_then(|t| dom::closest_of(&t, "button")).unwrap_or_else(|| btn.clone()));
                dom::spawn(async move { foryou::request_all(missing, btn).await });
            });
        }
    });
}

/// A taste of For you: its first songs, those in the library with Play.
fn for_you_view(tracks: Vec<Value>) -> impl IntoView {
    let rows = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let thumb = if t.truthy("library") {
                let song = t.g("library").clone();
                view! { <button class="thumb play-thumb" data-fy=i aria-label="Play" on:click=move |_| player::play_queue(vec![song.clone()], 0, "single") {..html(format!("{}<i>{}</i>", art(&t.s("cover"), &t.s("title"), ""), icon("play")))}>{""}</button> }.into_any()
            } else {
                view! { <div class="thumb" {..html(art(&t.s("cover"), &t.s("title"), ""))}>{""}</div> }.into_any()
            };
            let badge = t.truthy("library").then(|| view! { <span class="badge ok">"In library"</span> });
            view! {
                <div class="list-item">
                    {thumb}
                    <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{ui::tr_inline(t, "title")}</div><div class="small muted ellipsis">{ui::tr_inline(t, "artist")}</div></div>
                    {badge}</div>
            }
        })
        .collect_view();
    view! {
        <div class="section-head"><h2>"For you"</h2><a class="btn sm" href="#/for-you">"See all"</a></div>
        <div class="card flat"><div class="list">{rows}</div></div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The greeting follows the hour.
    #[test]
    fn the_greeting_follows_the_hour() {
        assert_eq!([0, 11, 12, 17, 18, 23].map(greeting), ["Good morning", "Good morning", "Good afternoon", "Good afternoon", "Good evening", "Good evening"]);
    }

    /// A pasted link's toast says what was requested.
    #[test]
    fn a_pasted_link_says_what_was_requested() {
        assert_eq!(link_toast(&json!({ "mode": "download" })), "Download started");
        assert_eq!(link_toast(&json!({ "mode": "matched", "requested": 3, "name": "Mix", "already_requested": 2 })), "3 song(s) requested from Mix, 2 already requested");
        assert_eq!(link_toast(&json!({ "mode": "matched", "requested": 1 })), "1 song(s) requested");
    }
}
