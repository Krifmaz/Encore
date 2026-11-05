//! Storage at a glance: disk, library breakdown, biggest artists/albums, download folders. The page is a Leptos
//! view ([`Storage`]); the sidebar's small meter ([`sidebar_meter`]) is still drawn as HTML.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{js_string, J};
use crate::ui::fmt;
use leptos::prelude::*;
use serde_json::Value;
use web_sys::Element;

/// `${n}` of a JS number (2.5 → "2.5", 100 → "100").
fn js_num(n: f64) -> String { js_string(&Value::from(n)) }

/// One size bar: `part` of `whole` (at most full) in `color`.
fn bar_view(part: f64, whole: f64, color: &str) -> impl IntoView {
    let width = if whole != 0.0 && !whole.is_nan() { (part / whole * 100.0).min(100.0) } else { 0.0 };
    view! { <div class="progress" style="height:8px"><i style=format!("width:{}%;background:{color}", js_num(width))></i></div> }
}

/// The page drawn from the answer of GET /api/library/storage: the disk, the library's totals, sizes by format
/// and by artist, and the biggest albums.
#[component]
pub fn Storage(
    /// The answer.
    data: Value,
) -> impl IntoView {
    let s = data;
    let artists = s.a("artists");
    let formats = s.a("formats");
    let max_a = artists.iter().map(|a| a.f("size")).fold(1.0_f64, f64::max);
    let max_f = formats.iter().map(|f| f.f("size")).fold(1.0_f64, f64::max);
    let library = s.g("library");
    let lib_size = library.f("size");
    let disk = s.truthy("disk").then(|| {
        let disk = s.g("disk");
        let (used, free, total) = (disk.f("used"), disk.f("free"), disk.f("total"));
        let other = (used - lib_size).max(0.0);
        view! {
            <div class="card mt"><div class="row between wrap"><b>"Disk"</b><span class="muted small">{format!("{} used · {} free of {}", fmt::size(used), fmt::size(free), fmt::size(total))}</span></div>
                <div class="progress mt" style="height:14px;display:flex"><i style=format!("width:{}%;background:var(--accent)", js_num(lib_size / total * 100.0)) title="Music"></i><i style=format!("width:{}%;background:var(--muted);opacity:.5", js_num(other / total * 100.0)) title="Other files"></i></div>
                <div class="row small muted mt" style="gap:16px"><span>"■ "<b style="color:var(--accent)">"Music"</b>{format!(" {}", fmt::size(lib_size))}</span><span>{format!("■ Other {}", fmt::size(other))}</span><span>{format!("□ Free {}", fmt::size(free))}</span></div>
            </div>
        }
    });
    let downloads = s
        .a("downloads")
        .iter()
        .map(|d| {
            let what = if d.g("name").as_str() == Some("soulseek") { "Soulseek downloads folder" } else { "Staging (in progress)" };
            view! { <div class="stat"><b>{fmt::size(d.f("size"))}</b><span>{what}</span></div> }
        })
        .collect_view();
    let by_format = formats
        .iter()
        .map(|f| {
            let name = if f.truthy("format") { f.s("format") } else { "?".to_string() };
            view! { <div><div class="row between small"><b>{name}</b><span class="muted">{format!("{} songs · {}", fmt::num(f.f("tracks")), fmt::size(f.f("size")))}</span></div>{bar_view(f.f("size"), max_f, "var(--accent)")}</div> }
        })
        .collect_view();
    let by_artist = artists
        .iter()
        .map(|a| {
            let name = a.s("name");
            let href = format!("#/artist-name/{}", String::from(js_sys::encode_uri_component(&name)));
            view! { <div><div class="row between small"><a href=href><b>{name}</b></a><span class="muted">{fmt::size(a.f("size"))}</span></div>{bar_view(a.f("size"), max_a, "var(--accent-2)")}</div> }
        })
        .collect_view();
    let albums = s
        .a("albums")
        .iter()
        .map(|a| {
            let album = if a.truthy("album") { a.s("album") } else { "Unknown album".to_string() };
            view! {
                <a class="list-item" href=format!("#/library/album/{}", a.s("id"))>
                    <div class="grow"><div style="font-weight:600" class="ellipsis">{album}</div><div class="small muted">{format!("{} · {} songs", a.s("albumartist"), a.s("tracks"))}</div></div><b>{fmt::size(a.f("size"))}</b>
                </a>
            }
        })
        .collect_view();
    view! {
        <h1>"Storage"</h1>
        {disk}
        <div class="stats mt">
            <div class="stat"><b>{fmt::size(lib_size)}</b><span>"Library"</span></div>
            <div class="stat"><b>{fmt::num(library.f("albums"))}</b><span>"Albums"</span></div>
            <div class="stat"><b>{fmt::num(library.f("tracks"))}</b><span>"Songs"</span></div>
            {downloads}
        </div>
        <div class="row wrap gap-lg mt" style="align-items:flex-start">
            <div class="card stack" style="flex:1;min-width:280px"><h2>"By format"</h2>{by_format}</div>
            <div class="card stack" style="flex:1;min-width:280px"><h2>"Biggest artists"</h2>{by_artist}</div>
        </div>
        <div class="section-head"><h2>"Biggest albums"</h2></div>
        <div class="card flat"><div class="list">{albums}</div></div>
    }
}

/// The Storage page: the heading and a skeleton, then the view once the answer has come.
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, r#"<h1>Storage</h1><div class="skeleton mt" style="height:200px"></div>"#);
        let data = api::get("/library/storage").await.map_err(|e| e.message)?;
        set_html(&view, "");
        // kept while this page is on screen; the next page takes it off
        if let Some(m) = kit_leptos::rt::mount(&view, move || view! { <Storage data/> }) {
            dom::keep(m);
        }
        Ok(())
    })
}

/// Tiny meter in the sidebar; refreshes every few minutes.
pub fn sidebar_meter(el: Element) {
    dom::spawn(async move {
        let Ok(s) = api::get("/library/storage").await else { set_html(&el, ""); return };
        let lib_size = s.g("library").f("size");
        if !s.truthy("disk") {
            set_html(&el, &format!(r##"<a href="#/storage" class="storage-mini"><span>Library {}</span></a>"##, fmt::size(lib_size)));
            return;
        }
        let disk = s.g("disk");
        let pct = disk.f("used") / disk.f("total") * 100.0;
        set_html(&el, &format!(r##"<a href="#/storage" class="storage-mini" title="Storage">
      <div class="row between small"><span>Storage</span><span class="faint">{} free</span></div>
      <div class="progress" style="height:6px;margin-top:6px"><i style="width:{}%;{}"></i></div>
      <div class="faint" style="font-size:11px;margin-top:4px">Music {} · {} disk</div></a>"##,
            fmt::size(disk.f("free")), js_num(pct), if pct > 90.0 { "background:var(--err)" } else { "" },
            fmt::size(lib_size), fmt::size(disk.f("total"))));
    });
}
