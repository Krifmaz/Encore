//! Port of web/js/pages/download.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::pages::{activity, common, playlists};
use crate::ui::{self, art, esc, icon, toast};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

// links that only describe tracks (no downloadable audio) → resolved track-by-track
const METADATA_ONLY: &str = r"open\.spotify\.com|spotify\.link|^spotify:|music\.apple\.com|music\.amazon\.|amazon\.[a-z.]+/music/player/|amzn\.to/|listenbrainz\.org";
// YouTube Music playlists are best matched track-by-track too (so albums can be upgraded to lossless)
const PREFER_IMPORT: &str = r"music\.youtube\.com\/(playlist|browse)";
const STREAMRIP: &str = r"qobuz\.com|tidal\.com|deezer\.com|deezer\.page\.link";

/// `/re/i.test(s)`.
fn test(re: &str, s: &str) -> bool { js_sys::RegExp::new(re, "i").test(s) }

/// `root.querySelector(sel)` – the JS would throw on a missing element, we return the router's error.
fn need(root: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(root, sel).ok_or_else(|| format!("{sel} not found")) }

/// The JS `load`: the 30 most recent url/track/album requests of this user.
async fn load_rows(list: &Element) -> Result<(), api::ApiError> {
    let rows: Vec<Value> = api::get("/requests?mine=true&limit=100").await?.items().into_iter()
        .filter(|r| matches!(r.g("kind").as_str(), Some("url" | "track" | "album"))).take(30).collect();
    dom::set_html(list, &if rows.is_empty() {
        r#"<div class="empty muted">Nothing yet – paste a link above.</div>"#.to_string()
    } else {
        format!(r#"<div class="list">{}</div>"#, rows.iter().map(|r| common::request_row(r, false)).collect::<String>())
    });
    Ok(())
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, &format!(r##"
    <h1>Downloader</h1>
    <p class="muted" style="margin:6px 0 20px">Paste a link from anywhere. <b>Spotify, Apple Music, YouTube Music, Deezer, Tidal, Qobuz, Amazon Music</b> songs, albums and playlists are matched song by song (exact ISRC when available) and fetched from the best source you have – lossless first.
      <b>YouTube, SoundCloud, Bandcamp, Mixcloud</b> and ~1,800 other sites download directly. Every file is fingerprint-verified and de-duplicated before it hits the library.</p>
    <div id="storage-banner"></div>
    <form class="card stack" id="dl-form">
      <div class="row wrap">
        <div class="search grow">{link}<input class="input" name="url" placeholder="https://…" required autocomplete="off"></div>
        <button class="btn" type="button" id="probe">Preview</button>
      </div>
      <div class="row wrap">
        <span class="small muted">If a song is on an album:</span>
        <div class="tabs" id="album-mode">
          <button type="button" data-m="song">Just the song</button>
          <button type="button" data-m="album">Get the whole album</button>
          <button type="button" data-m="">Admin default</button>
        </div>
      </div>
      <div class="row wrap" id="dl-actions">
        <button class="btn primary" type="submit" id="get-songs" title="Works with any link: matched to the best source, or downloaded directly">{download}Download</button>
        <button class="btn" type="button" id="as-playlist">{list}Import as playlist</button>
        <button class="btn ghost" type="button" id="direct" title="Skip matching and download exactly this link">{link}Exactly this link</button>
      </div>
      <div id="probe-out"></div>
    </form>
    <div class="section-head"><h2>Recent downloads</h2><div class="row" style="gap:8px"><button class="btn sm" id="dl-retry-failed" title="Queue every failed download again">{refresh}Retry failed</button><button class="btn sm" id="dl-clear">{trash}Clear…</button></div></div>
    <div class="card flat" id="dl-list"></div>"##,
            link = icon("link"), download = icon("download"), list = icon("list"), trash = icon("trash"), refresh = icon("refresh")));
        let form = need(&view, "#dl-form")?;
        let out = need(&view, "#probe-out")?;
        let url_in = need(&form, "input[name=url]")?;
        let direct = need(&view, "#direct")?;
        {
            // update()
            let (u, d) = (url_in.clone(), direct.clone());
            dom::on(&url_in, "input", move |_| {
                let s = dom::value(&u).trim().to_string();
                let disabled = test(METADATA_ONLY, &s);
                dom::set_disabled(&d, disabled);
                let _ = d.set_attribute("title", if disabled { "This service has no downloadable audio – use “Import as playlist”" } else if test(STREAMRIP, &s) { "Rip this exact release with streamrip" } else { "" });
            });
        }
        let _ = PREFER_IMPORT; // void PREFER_IMPORT;

        let album_mode: Rc<RefCell<String>> = Rc::new(RefCell::new(dom::ls_get("encore.albumMode").unwrap_or_else(|| "song".into())));
        let paint_mode: Rc<dyn Fn()> = {
            let (v, m) = (view.clone(), album_mode.clone());
            Rc::new(move || {
                for b in dom::qsa_in(&v, "#album-mode button") { dom::toggle_class(&b, "active", dom::data_opt(&b, "m").as_deref() == Some(m.borrow().as_str())); }
            })
        };
        paint_mode();
        {
            let (m, p) = (album_mode.clone(), paint_mode.clone());
            dom::on_sel(&view, "#album-mode", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-m]") else { return };
                *m.borrow_mut() = dom::data(&b, "m");
                dom::ls_set("encore.albumMode", &m.borrow());
                p();
            });
        }

        // load() – defined before the handlers that call it (the JS hoisted it through its closures)
        let list = need(&view, "#dl-list")?;
        let load: Rc<dyn Fn()> = {
            let l = list.clone();
            Rc::new(move || { let l = l.clone(); dom::spawn(async move { let _ = load_rows(&l).await; }); })
        };

        // smart() – form.onsubmit
        {
            let (v, u, o, m, l) = (view.clone(), url_in.clone(), out.clone(), album_mode.clone(), load.clone());
            dom::on(&form, "submit", move |e| {
                dom::prevent(e);
                let url = dom::value(&u).trim().to_string();
                if url.is_empty() { dom::focus(&u); return; }
                let Some(btn) = dom::qs_in(&v, "#get-songs") else { return };
                dom::set_disabled(&btn, true);
                dom::set_html(&btn, r#"<span class="spinner"></span> Looking up…"#);
                let (u, o, m, l) = (u.clone(), o.clone(), m.clone(), l.clone());
                dom::spawn(async move {
                    let album_mode = m.borrow().clone();
                    let album_mode_val = if album_mode.is_empty() { Value::Null } else { Value::String(album_mode) };
                    match api::post("/requests/link", json!({ "url": url, "album_mode": album_mode_val })).await {
                        Ok(r) => {
                            toast(&if r.s("mode") == "direct" { "Downloading".to_string() } else {
                                format!("{} song(s) requested{}{}", r.s("requested"),
                                    if r.truthy("already_requested") { format!(", {} already requested", r.s("already_requested")) } else { String::new() },
                                    if *m.borrow() == "album" { " – whole albums will be fetched" } else { "" })
                            }, "ok");
                            dom::set_value(&u, "");
                            dom::set_html(&o, "");
                            dom::dispatch("encore:requests-changed", None);
                            l();
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&btn, false);
                    dom::set_html(&btn, &format!("{}Download", icon("download")));
                });
            });
        }

        {
            let (u, m) = (url_in.clone(), album_mode.clone());
            dom::on_sel(&view, "#as-playlist", "click", move |_| {
                let url = dom::value(&u).trim().to_string();
                if url.is_empty() { dom::focus(&u); return; }
                playlists::import_modal(&url, None);
                let m = m.clone();
                dom::timeout(50, move || {
                    if let Some(req) = dom::qs("#imp-req") { dom::set_checked(&req, true); }
                    if let Some(sel) = dom::qs("#imp-album") { dom::set_value(&sel, &m.borrow()); }
                    if let Some(p) = dom::qs("#imp-preview") { dom::click(&p); }
                });
            });
        }

        {
            let (v, u, o) = (view.clone(), url_in.clone(), out.clone());
            dom::on_sel(&view, "#probe", "click", move |_| {
                let url = dom::value(&u).trim().to_string();
                if url.is_empty() { return; }
                if test(METADATA_ONLY, &url) || test(STREAMRIP, &url) {
                    if let Some(b) = dom::qs_in(&v, "#as-playlist") { dom::click(&b); }
                    return;
                }
                dom::set_html(&o, r#"<span class="spinner"></span>"#);
                let o = o.clone();
                dom::spawn(async move {
                    match api::get(&format!("/discover/probe?url={}", String::from(js_sys::encode_uri_component(&url)))).await {
                        Ok(p) => {
                            let title = p.s("title");
                            let sub = [
                                p.s("extractor"),
                                p.s("uploader"),
                                if p.truthy("is_playlist") { format!("{} items", p.s("count")) } else if p.truthy("duration") { ui::fmt::dur(p.f("duration")) } else { String::new() },
                            ].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                            dom::set_html(&o, &format!(r#"<div class="row">
        <div class="thumb lg">{}</div>
        <div class="grow"><b>{}</b>
          <div class="small muted">{}</div>
        </div></div>"#, art(&p.s("thumbnail"), &title, ""), esc(if title.is_empty() { "Untitled" } else { &title }), esc(&sub)));
                        }
                        Err(e) => dom::set_html(&o, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))),
                    }
                });
            });
        }

        {
            let (v, u, o, d, l) = (view.clone(), url_in.clone(), out.clone(), direct.clone(), load.clone());
            dom::on(&direct, "click", move |e| {
                dom::prevent(e);
                let url = dom::value(&u).trim().to_string();
                if url.is_empty() { dom::focus(&u); return; }
                if test(METADATA_ONLY, &url) {
                    if let Some(b) = dom::qs_in(&v, "#as-playlist") { dom::click(&b); }
                    return;
                }
                dom::set_disabled(&d, true);
                let title = dom::qs_in(&o, "b").map(|b| dom::text_of(&b)).filter(|t| !t.is_empty()).map(Value::String).unwrap_or(Value::Null);
                let (u, o, d, l) = (u.clone(), o.clone(), d.clone(), l.clone());
                dom::spawn(async move {
                    // submitRequest never throws (it toasts and returns null itself), so the JS catch is unreachable
                    let _ = common::submit_request(json!({ "kind": "url", "url": url, "title": title }), None).await;
                    dom::set_value(&u, "");
                    dom::set_html(&o, "");
                    l();
                    dom::set_disabled(&d, false);
                });
            });
        }

        common::bind_request_actions(&list, load.clone());
        // imports paused because a folder can't be written (R1): the same banner as on Requests
        if let Some(slot) = dom::qs_in(&view, "#storage-banner") {
            common::storage_banner(&view, &slot, matches!(ctx.user.s("role").as_str(), "admin" | "owner"));
        }
        {
            let clear = need(&view, "#dl-clear")?;
            let (c, l) = (clear.clone(), load.clone());
            dom::on(&clear, "click", move |_| common::clear_menu(&c, l.clone(), true));
        }
        {
            let retry = need(&view, "#dl-retry-failed")?;
            let l = load.clone();
            dom::on(&retry, "click", move |_| { let l = l.clone(); common::retry_failed(true, l); });
        }
        load_rows(&list).await.map_err(|e| e.message)?;
        {
            let l = load.clone();
            dom::interval(&view, 4000, move || l());
        }
        if matches!(ctx.user.s("role").as_str(), "admin" | "owner") {
            // transfers, torrent/Usenet queue and the server log live here too (was the Activity page)
            let act = dom::create("div");
            act.set_class_name("embedded mt-lg");
            let _ = view.append_child(&act);
            // the JS did not await activity's render
            dom::spawn(async move { let _ = activity::render(act, ctx).await; });
        }
        Ok(())
    })
}
