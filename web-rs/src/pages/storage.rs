//! Port of web/js/pages/storage.js – see docs/ui-rewrite/CONTRACT.md
//! Storage at a glance: disk, library breakdown, biggest artists/albums, download folders.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{js_string, J};
use crate::ui::{esc, fmt};
use serde_json::Value;
use web_sys::Element;

/// `${n}` of a JS number (2.5 → "2.5", 100 → "100").
fn js_num(n: f64) -> String { js_string(&Value::from(n)) }

/// `bar(part, whole, color = 'var(--accent)')`.
fn bar(part: f64, whole: f64, color: &str) -> String {
    let width = if whole != 0.0 && !whole.is_nan() { (part / whole * 100.0).min(100.0) } else { 0.0 };
    format!(r#"<div class="progress" style="height:8px"><i style="width:{}%;background:{color}"></i></div>"#, js_num(width))
}

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, r#"<h1>Storage</h1><div class="skeleton mt" style="height:200px"></div>"#);
        let s = api::get("/library/storage").await.map_err(|e| e.message)?;
        let artists = s.a("artists");
        let formats = s.a("formats");
        let max_a = artists.iter().map(|a| a.f("size")).fold(1.0_f64, f64::max);
        let max_f = formats.iter().map(|f| f.f("size")).fold(1.0_f64, f64::max);
        let library = s.g("library");
        let lib_size = library.f("size");

        let disk_html = if s.truthy("disk") {
            let disk = s.g("disk");
            let (used, free, total) = (disk.f("used"), disk.f("free"), disk.f("total"));
            let other = (used - lib_size).max(0.0);
            format!(r#"<div class="card mt"><div class="row between wrap"><b>Disk</b><span class="muted small">{} used · {} free of {}</span></div>
      <div class="progress mt" style="height:14px;display:flex"><i style="width:{}%;background:var(--accent)" title="Music"></i><i style="width:{}%;background:var(--muted);opacity:.5" title="Other files"></i></div>
      <div class="row small muted mt" style="gap:16px"><span>■ <b style="color:var(--accent)">Music</b> {}</span><span>■ Other {}</span><span>□ Free {}</span></div></div>"#,
                fmt::size(used), fmt::size(free), fmt::size(total),
                js_num(lib_size / total * 100.0), js_num(other / total * 100.0),
                fmt::size(lib_size), fmt::size(other), fmt::size(free))
        } else { String::new() };

        let downloads_html: String = s.a("downloads").iter().map(|d| format!(r#"<div class="stat"><b>{}</b><span>{}</span></div>"#,
            fmt::size(d.f("size")),
            if d.g("name").as_str() == Some("soulseek") { "Soulseek downloads folder" } else { "Staging (in progress)" })).collect();

        let formats_html: String = formats.iter().map(|f| {
            let name = if f.truthy("format") { f.s("format") } else { "?".to_string() };
            format!(r#"<div><div class="row between small"><b>{}</b><span class="muted">{} songs · {}</span></div>{}</div>"#,
                esc(&name), fmt::num(f.f("tracks")), fmt::size(f.f("size")), bar(f.f("size"), max_f, "var(--accent)"))
        }).collect();

        let artists_html: String = artists.iter().map(|a| {
            let name = a.s("name");
            format!(r##"<div><div class="row between small"><a href="#/artist-name/{}"><b>{}</b></a><span class="muted">{}</span></div>{}</div>"##,
                String::from(js_sys::encode_uri_component(&name)), esc(&name), fmt::size(a.f("size")), bar(a.f("size"), max_a, "var(--accent-2)"))
        }).collect();

        let albums_html: String = s.a("albums").iter().map(|a| {
            let album = if a.truthy("album") { a.s("album") } else { "Unknown album".to_string() };
            format!(r##"<a class="list-item" href="#/library/album/{}">
      <div class="grow"><div style="font-weight:600" class="ellipsis">{}</div><div class="small muted">{} · {} songs</div></div><b>{}</b></a>"##,
                esc(&a.s("id")), esc(&album), esc(&a.s("albumartist")), a.s("tracks"), fmt::size(a.f("size")))
        }).collect();

        set_html(&view, &format!(r#"
    <h1>Storage</h1>
    {disk_html}
    <div class="stats mt">
      <div class="stat"><b>{}</b><span>Library</span></div>
      <div class="stat"><b>{}</b><span>Albums</span></div>
      <div class="stat"><b>{}</b><span>Songs</span></div>
      {downloads_html}
    </div>
    <div class="row wrap gap-lg mt" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:280px"><h2>By format</h2>
        {formats_html}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Biggest artists</h2>
        {artists_html}</div>
    </div>
    <div class="section-head"><h2>Biggest albums</h2></div>
    <div class="card flat"><div class="list">{albums_html}</div></div>"#,
            fmt::size(lib_size), fmt::num(library.f("albums")), fmt::num(library.f("tracks"))));
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
