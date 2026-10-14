//! Port of web/js/pages/leaderboard.js – see docs/ui-rewrite/CONTRACT.md
//! Leaderboard: who requests, plays and likes the most, plus fun library facts.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{art_sized, empty_state, esc, fmt, icon};
use serde_json::Value;
use web_sys::Element;

const MEDAL: [&str; 3] = ["🥇", "🥈", "🥉"];

/// `MEDAL[i] || (i + 1) + '.'`
fn medal(i: usize) -> String {
    MEDAL.get(i).map(|m| m.to_string()).unwrap_or_else(|| format!("{}.", i + 1))
}

/// `encodeURIComponent(s)`
fn encode(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// `fmtV(k, v)`
fn fmt_v(k: &str, v: f64) -> String { if k == "listened" { fmt::long_dur(v) } else { fmt::num(v) } }

/// `board(k, b)` – one ranking card (the whitespace inside the strings is the JS template literal's).
fn board(k: &str, b: &Value) -> String {
    let rows = b.a("rows");
    format!(r#"<div class="card stack" style="flex:1;min-width:260px"><h2>{}</h2>
    {}</div>"#, esc(&b.s("label")),
        if !rows.is_empty() {
            rows.iter().enumerate().map(|(i, r)| {
                let me = r.truthy("me");
                format!(r#"<div class="row between" style="{}">
      <span>{} {}{}</span><b>{}</b></div>"#,
                    if me { "color:var(--accent);font-weight:700" } else { "" },
                    MEDAL.get(i).map(|m| m.to_string()).unwrap_or_else(|| format!(r#"<span class="faint" style="display:inline-block;width:22px">{}.</span>"#, i + 1)),
                    esc(&r.s("username")),
                    if me { " (you)" } else { "" },
                    fmt_v(k, r.f("value")))
            }).collect::<String>()
        } else {
            r#"<div class="muted small">Nobody yet – be the first!</div>"#.to_string()
        })
}

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, r#"<h1>Leaderboard</h1><div class="skeleton mt" style="height:240px"></div>"#);
        let d = api::get("/stats/leaderboard").await.map_err(|e| e.message)?;
        let boards = d.g("boards").as_object().map(|o| o.iter().map(|(k, b)| board(k, b)).collect::<String>()).unwrap_or_default();
        let this_week = d.a("this_week");
        let week = match this_week.first() {
            Some(w) => format!(r#"<div class="section-head"><h2>This week's top listener</h2></div><div class="card"><b style="font-size:20px">👑 {}</b> <span class="muted">· {} songs played this week</span></div>"#,
                esc(&w.s("username")), fmt::num(w.f("plays"))),
            None => String::new(),
        };
        let top_songs = d.a("top_songs");
        let songs = if !top_songs.is_empty() {
            top_songs.iter().enumerate().map(|(i, s)| format!(r#"<div class="row" style="gap:10px"><span class="faint" style="width:20px">{}</span>
          <div class="thumb" style="width:36px;height:36px">{}</div>
          <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div></div><b>{}</b></div>"#,
                i + 1,
                art_sized(&if s.truthy("album_id") { format!("/api/library/albums/{}/cover{}", s.s("album_id"), crate::ui::cover_q(s)) } else { String::new() }, &s.s("title"), "", 200),
                esc(&s.s("title")), esc(&s.s("artist")), fmt::num(s.f("plays")))).collect::<String>()
        } else {
            empty_state(&icon("play"), "No plays yet", "")
        };
        let top_artists = d.a("top_artists");
        let artists = if !top_artists.is_empty() {
            top_artists.iter().enumerate().map(|(i, a)| format!(r##"<div class="row between"><span>{} <a href="#/artist-name/{}">{}</a> <span class="faint small">· {} listener{}</span></span><b>{}</b></div>"##,
                medal(i), encode(&a.s("artist")), esc(&a.s("artist")), a.s("listeners"),
                if a.g("listeners").as_f64() == Some(1.0) { "" } else { "s" }, fmt::num(a.f("plays")))).collect::<String>()
        } else {
            r#"<div class="muted small">No plays yet.</div>"#.to_string()
        };
        let library = d.a("library_artists").iter().enumerate().map(|(i, a)| format!(r##"<div class="row between"><span>{} <a href="#/artist-name/{}">{}</a></span><span class="muted small">{} albums · <b>{}</b> songs</span></div>"##,
            medal(i), encode(&a.s("artist")), esc(&a.s("artist")), a.s("albums"), fmt::num(a.f("songs")))).collect::<String>();
        let facts = d.a("facts").iter().map(|f| format!(r#"<div class="stat"><b>{}</b><span>{}</span></div>"#, esc(&f.s("value")), esc(&f.s("label")))).collect::<String>();
        dom::set_html(&view, &format!(r#"
    <h1>Leaderboard</h1><p class="muted" style="margin:6px 0 20px">Plays count after 30 seconds in the web player. Bragging rights only.</p>
    <div class="row wrap gap-lg" style="align-items:flex-start">{boards}</div>
    {week}
    <div class="row wrap gap-lg mt" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:280px"><h2>Most played songs</h2>
        {songs}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Most played artists</h2>
        {artists}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Biggest in the library</h2>
        {library}</div>
    </div>
    <div class="section-head"><h2>Fun facts</h2></div>
    <div class="stats">{facts}</div>"#));
        Ok(())
    })
}
