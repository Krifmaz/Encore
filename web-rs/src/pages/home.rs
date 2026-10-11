//! Port of web/js/pages/home.js – see docs/ui-rewrite/CONTRACT.md
use crate::api::{self, ApiError};
use crate::app::{self, Ctx, PageFuture};
use crate::dom::{self, EventListener};
use crate::json::{json, J};
use crate::pages::common::{bind_request_actions, bind_tiles, library_tile, request_row};
use crate::player;
use crate::ui::{self, art, empty_state, esc, icon, toast};
use gloo_events::EventListenerOptions;
use serde_json::Value;
use std::rc::Rc;
use web_sys::{Element, Event};

/// `form.onsubmit = f`. gloo's default listener is passive (`preventDefault()` would be ignored and the
/// browser would submit the form), so the submit handlers are registered non-passive.
fn on_submit<F: FnMut(&Event) + 'static>(form: &Element, f: F) {
    dom::keep(EventListener::new_with_options(form, "submit", EventListenerOptions::enable_prevent_default(), f));
}

/// `loadReqs`: the user's active downloads into #reqs; the wrap stays hidden while there are none.
async fn load_reqs(view: &Element) -> Result<(), ApiError> {
    let rows = api::get("/requests?status=active&mine=true&limit=8").await?.items();
    if let Some(wrap) = dom::qs_in(view, "#reqs-wrap") { dom::toggle_class(&wrap, "hidden", rows.is_empty()); }
    if let Some(reqs) = dom::qs_in(view, "#reqs") {
        dom::set_html(&reqs, &if rows.is_empty() { String::new() } else {
            format!(r#"<div class="list">{}</div>"#, rows.iter().map(|r| request_row(r, false)).collect::<String>())
        });
    }
    Ok(())
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let hour = js_sys::Date::new_0().get_hours();
        let greet = if hour < 12 { "Good morning" } else if hour < 18 { "Good afternoon" } else { "Good evening" };
        dom::set_html(&view, &format!(r##"
    <section class="hero">
      <h1>{greet}, {username}</h1>
      <p class="muted" style="margin:6px 0 0">Find any artist, album or song – or paste a link from anywhere.</p>
      <form class="search" id="home-search">{search}<input class="input" name="q" type="search" enterkeyhint="search" placeholder="Search artists, albums, songs…" autocomplete="off"></form>
      <form class="search" id="home-link" style="margin-top:10px">{link}<input class="input" name="url" type="url" enterkeyhint="go" placeholder="…or paste a song, album or playlist link to download" autocomplete="off"></form>
    </section>
    <div class="stats mt" id="stats"></div>
    <div id="new-releases"></div>
    <div id="fy-preview"></div>
    <div id="reqs-wrap" class="hidden"><div class="section-head"><h2>Your downloads in progress</h2><a class="btn sm" href="#/requests">All requests</a></div>
      <div class="card flat" id="reqs"></div></div>
    <div class="section-head"><h2>Recently added</h2><a class="btn sm" href="#/library?tab=albums&sort=added&dir=desc">Library</a></div>
    <div id="recent"></div>"##,
            username = esc(&ctx.user.s("username")), search = icon("search"), link = icon("link")));

        // `loadReqs` as the callback the JS passed around (not awaited there, so errors are dropped like an unhandled rejection)
        let reload: Rc<dyn Fn()> = { let v = view.clone(); Rc::new(move || { let v = v.clone(); dom::spawn(async move { let _ = load_reqs(&v).await; }); }) };

        if let Some(form) = dom::qs_in(&view, "#home-search") {
            let f = form.clone();
            on_submit(&form, move |e| {
                dom::prevent(e);
                let q = dom::value_of(&f, "[name=q]").trim().to_string();
                if !q.is_empty() { app::navigate(&format!("/search?q={}", String::from(js_sys::encode_uri_component(&q)))); }
            });
        }
        if let Some(form) = dom::qs_in(&view, "#home-link") {
            let f = form.clone();
            let reload = reload.clone();
            on_submit(&form, move |e| {
                dom::prevent(e);
                let Some(input) = dom::qs_in(&f, "input") else { return };
                let url = dom::value(&input).trim().to_string();
                if url.is_empty() { return; }
                dom::set_disabled(&input, true);
                let reload = reload.clone();
                dom::spawn(async move {
                    match api::post("/requests/link", json!({ "url": url })).await {
                        Ok(r) => {
                            toast(&if r.s("mode") == "matched" {
                                format!("{} song(s) requested{}{}", r.s("requested"),
                                    if r.truthy("name") { format!(" from {}", r.s("name")) } else { String::new() },
                                    if r.truthy("already_requested") { format!(", {} already requested", r.s("already_requested")) } else { String::new() })
                            } else { "Download started".to_string() }, "ok");
                            dom::set_value(&input, "");
                            dom::dispatch("encore:requests-changed", None);
                            reload();
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&input, false);
                });
            });
        }
        bind_tiles(&view);

        if let Some(reqs) = dom::qs_in(&view, "#reqs") { bind_request_actions(&reqs, reload.clone()); }

        // fun stats
        {
            let v = view.clone();
            dom::spawn(async move {
                let Ok(s) = api::get("/stats/me").await else { return };
                let (lib, me) = (s.g("library"), s.g("me"));
                // the JS threw (and swallowed it) on `s.library.tracks` / `s.me.plays` without these objects
                if !lib.is_object() || !me.is_object() { return; }
                let Some(el) = dom::qs_in(&v, "#stats") else { return };
                let duration = lib.f("duration");
                dom::set_html(&el, &format!(r##"
      <div class="stat"><b>{tracks}</b><span>songs in the library</span></div>
      <div class="stat"><b>{long_dur}</b><span>to listen to{days}</span></div>
      <div class="stat"><b>{plays}</b><span>songs you played{listened}</span></div>
      <div class="stat"><b>{requests}</b><span>things you requested{rank}</span></div>
      {top_artist}
      {fun}"##,
                    tracks = ui::fmt::num(lib.f("tracks")),
                    long_dur = ui::fmt::long_dur(duration),
                    days = if duration > 86400.0 { format!(" · {:.1} days non-stop", duration / 86400.0) } else { String::new() },
                    plays = ui::fmt::num(me.f("plays")),
                    listened = if me.truthy("listened") { format!(" · {}", ui::fmt::long_dur(me.f("listened"))) } else { String::new() },
                    requests = ui::fmt::num(me.f("requests")),
                    rank = if me.truthy("rank") { format!(r##" · #{} on the <a href="#/leaderboard">leaderboard</a>"##, me.s("rank")) } else { String::new() },
                    top_artist = if me.truthy("top_artist") { format!(r#"<div class="stat"><b class="ellipsis" style="display:block">{}</b><span>your most played artist</span></div>"#, esc(&me.s("top_artist"))) } else { String::new() },
                    fun = if s.truthy("fun") { let fun = s.g("fun"); format!(r#"<div class="stat"><b>{}</b><span>{}</span></div>"#, esc(&fun.s("value")), esc(&fun.s("label"))) } else { String::new() },
                ));
            });
        }

        // new releases from library artists
        {
            let v = view.clone();
            dom::spawn(async move {
                let Ok(d) = api::get("/discover/new-releases?limit=12").await else { return };
                let releases = d.a("releases");
                if releases.is_empty() || !v.is_connected() { return; }
                let Some(el) = dom::qs_in(&v, "#new-releases") else { return };
                dom::set_html(&el, &format!(r#"<div class="section-head"><h2>New releases</h2><span class="small muted">from artists in your library</span></div>
      <div class="grid">{}</div>"#, releases.iter().map(|r| format!(r#"<div class="tile" data-href="{href}">
        <div class="art">{art}
          <div class="corner">{corner}</div>
</div>
        <div class="t ellipsis" title="{title}">{title}</div>
        <div class="s ellipsis">{artist} · {date}{kind}</div></div>"#,
                    href = if r.truthy("mbid") { format!("/album/{}", esc(&r.s("mbid"))) } else { format!("/artist-name/{}", String::from(js_sys::encode_uri_component(&r.s("artist")))) },
                    art = art(&r.s("cover"), &r.s("title"), ""),
                    corner = if r.truthy("in_library") { r#"<span class="badge ok">In library</span>"# } else { "" },
                    title = esc(&r.s("title")),
                    artist = esc(&r.s("artist")),
                    date = esc(&r.s("date")),
                    kind = if r.truthy("type") && r.s("type") != "Album" { format!(" · {}", esc(&r.s("type"))) } else { String::new() },
                )).collect::<String>()));
            });
        }

        // a taste of For you
        {
            let v = view.clone();
            dom::spawn(async move {
                let Ok(d) = api::get("/discover/for-you").await else { return };
                let Some(pl) = d.a("playlists").into_iter().next().filter(crate::json::truthy) else { return };
                if !v.is_connected() { return; }
                // the JS threw (and swallowed it) on `pl.tracks.slice` without a tracks array
                if !pl.g("tracks").is_array() { return; }
                let tr: Vec<Value> = pl.a("tracks").into_iter().take(6).collect();
                let Some(el) = dom::qs_in(&v, "#fy-preview") else { return };
                dom::set_html(&el, &format!(r##"<div class="section-head"><h2>For you</h2><a class="btn sm" href="#/for-you">See all</a></div>
      <div class="card flat"><div class="list">{}</div></div>"##, tr.iter().enumerate().map(|(i, t)| format!(r#"<div class="list-item">
        {thumb}
        <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{title}</div><div class="small muted ellipsis">{artist}</div></div>
        {badge}</div>"#,
                    thumb = if t.truthy("library") {
                        format!(r#"<button class="thumb play-thumb" data-fy="{i}" aria-label="Play">{}<i>{}</i></button>"#, art(&t.s("cover"), &t.s("title"), ""), icon("play"))
                    } else {
                        format!(r#"<div class="thumb">{}</div>"#, art(&t.s("cover"), &t.s("title"), ""))
                    },
                    title = esc(&t.s("title")),
                    artist = esc(&t.s("artist")),
                    badge = if t.truthy("library") { r#"<span class="badge ok">In library</span>"# } else { "" },
                )).collect::<String>()));
                let tr = Rc::new(tr);
                dom::on(&el, "click", move |e| {
                    let Some(b) = dom::closest(e, "[data-fy]") else { return };
                    let Some(t) = dom::data(&b, "fy").parse::<usize>().ok().and_then(|i| tr.get(i)) else { return };
                    if t.truthy("library") { player::play_queue(vec![t.g("library").clone()], 0, "single"); }
                });
            });
        }

        let (recent, reqs) = futures::join!(api::get("/library/recent?limit=12"), load_reqs(&view));
        let recent = recent.map_err(|e| e.message)?.items();
        reqs.map_err(|e| e.message)?;
        if let Some(el) = dom::qs_in(&view, "#recent") {
            dom::set_html(&el, &if recent.is_empty() {
                empty_state(&icon("library"), "Your library is empty", "Search above or paste a link to get started.")
            } else {
                format!(r#"<div class="grid">{}</div>"#, recent.iter().map(library_tile).collect::<String>())
            });
        }
        // setInterval(loadReqs, 5000) + ctx.onLeave(clearInterval)
        let r = reload.clone();
        dom::interval(&view, 5000, move || r());
        Ok(())
    })
}
