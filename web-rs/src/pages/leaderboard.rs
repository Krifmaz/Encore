//! Leaderboard: who requests, plays and likes the most, plus fun library facts (GET /api/stats/leaderboard).
//!
//! A Leptos view ([`Leaderboard`]) drawn from the answer, mounted into the page once it has come; until then the
//! page shows its heading and a skeleton.

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{art_sized, empty_state, fmt, icon};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use web_sys::Element;

/// The first three places.
const MEDAL: [&str; 3] = ["🥇", "🥈", "🥉"];

/// The place of row `i`: a medal for the first three, "4." and on after them.
fn medal(i: usize) -> String {
    MEDAL.get(i).map(|m| m.to_string()).unwrap_or_else(|| format!("{}.", i + 1))
}

/// `encodeURIComponent(s)`: an artist's name in a link.
fn encode(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

/// A board's value: listening time as a duration, anything else as a number.
fn fmt_v(k: &str, v: f64) -> String {
    if k == "listened" { fmt::long_dur(v) } else { fmt::num(v) }
}

/// One ranking card (requests, plays, likes, listening time): its rows with the reader's own in the accent.
fn board(k: &str, b: &Value) -> impl IntoView {
    let rows = b.a("rows");
    let body = if rows.is_empty() {
        view! { <div class="muted small">"Nobody yet – be the first!"</div> }.into_any()
    } else {
        rows.iter()
            .enumerate()
            .map(|(i, r)| {
                let me = r.truthy("me");
                // a medal for the first three; after them the place in a fixed-width column, so the names line up
                let place = match MEDAL.get(i) {
                    Some(m) => m.to_string().into_any(),
                    None => view! { <span class="faint" style="display:inline-block;width:22px">{format!("{}.", i + 1)}</span> }.into_any(),
                };
                view! {
                    <div class="row between" style=if me { "color:var(--accent);font-weight:700" } else { "" }>
                        <span>{place}" "{r.s("username")}{if me { " (you)" } else { "" }}</span><b>{fmt_v(k, r.f("value"))}</b>
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };
    view! { <div class="card stack" style="flex:1;min-width:260px"><h2>{b.s("label")}</h2>{body}</div> }
}

/// The whole page drawn from the answer of GET /api/stats/leaderboard.
#[component]
pub fn Leaderboard(
    /// The answer.
    data: Value,
) -> impl IntoView {
    let boards = data.g("boards").as_object().map(|o| o.iter().map(|(k, b)| board(k, b)).collect_view());
    let week = data.a("this_week").first().map(|w| {
        view! {
            <div class="section-head"><h2>"This week's top listener"</h2></div>
            <div class="card"><b style="font-size:20px">{format!("👑 {}", w.s("username"))}</b>" "<span class="muted">{format!("· {} songs played this week", fmt::num(w.f("plays")))}</span></div>
        }
    });
    let top_songs = data.a("top_songs");
    let songs = if top_songs.is_empty() {
        raw(empty_state(&icon("play"), "No plays yet", "")).into_any()
    } else {
        top_songs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let cover = if s.truthy("album_id") { format!("/api/library/albums/{}/cover{}", s.s("album_id"), crate::ui::cover_q(s)) } else { String::new() };
                view! {
                    <div class="row" style="gap:10px"><span class="faint" style="width:20px">{i + 1}</span>
                        <div class="thumb" style="width:36px;height:36px" {..html(art_sized(&cover, &s.s("title"), "", 200))}>{""}</div>
                        <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{s.s("title")}</div><div class="small muted ellipsis">{s.s("artist")}</div></div><b>{fmt::num(s.f("plays"))}</b>
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };
    let top_artists = data.a("top_artists");
    let artists = if top_artists.is_empty() {
        view! { <div class="muted small">"No plays yet."</div> }.into_any()
    } else {
        top_artists
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let listeners = format!("· {} listener{}", a.s("listeners"), if a.g("listeners").as_f64() == Some(1.0) { "" } else { "s" });
                view! {
                    <div class="row between"><span>{format!("{} ", medal(i))}<a href=format!("#/artist-name/{}", encode(&a.s("artist")))>{a.s("artist")}</a>" "<span class="faint small">{listeners}</span></span><b>{fmt::num(a.f("plays"))}</b></div>
                }
            })
            .collect_view()
            .into_any()
    };
    let library = data
        .a("library_artists")
        .iter()
        .enumerate()
        .map(|(i, a)| {
            view! {
                <div class="row between"><span>{format!("{} ", medal(i))}<a href=format!("#/artist-name/{}", encode(&a.s("artist")))>{a.s("artist")}</a></span><span class="muted small">{format!("{} albums · ", a.s("albums"))}<b>{fmt::num(a.f("songs"))}</b>" songs"</span></div>
            }
        })
        .collect_view();
    let facts = data.a("facts").iter().map(|f| view! { <div class="stat"><b>{f.s("value")}</b><span>{f.s("label")}</span></div> }).collect_view();
    view! {
        <h1>"Leaderboard"</h1><p class="muted" style="margin:6px 0 20px">"Plays count after 30 seconds in the web player. Bragging rights only."</p>
        <div class="row wrap gap-lg" style="align-items:flex-start">{boards}</div>
        {week}
        <div class="row wrap gap-lg mt" style="align-items:flex-start">
            <div class="card stack" style="flex:1;min-width:280px"><h2>"Most played songs"</h2>{songs}</div>
            <div class="card stack" style="flex:1;min-width:280px"><h2>"Most played artists"</h2>{artists}</div>
            <div class="card stack" style="flex:1;min-width:280px"><h2>"Biggest in the library"</h2>{library}</div>
        </div>
        <div class="section-head"><h2>"Fun facts"</h2></div>
        <div class="stats">{facts}</div>
    }
}

/// The Leaderboard page: the heading and a skeleton, then the view once the answer has come.
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, r#"<h1>Leaderboard</h1><div class="skeleton mt" style="height:240px"></div>"#);
        let data = api::get("/stats/leaderboard").await.map_err(|e| e.message)?;
        dom::set_html(&view, "");
        // kept while this page is on screen; the next page takes it off
        if let Some(m) = kit_leptos::rt::mount(&view, move || view! { <Leaderboard data/> }) {
            dom::keep(m);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The places read as before: medals for the first three, then "4." and on.
    #[test]
    fn the_places_are_medals_then_numbers() {
        assert_eq!((0..5).map(medal).collect::<Vec<_>>(), ["🥇", "🥈", "🥉", "4.", "5."]);
    }
}
