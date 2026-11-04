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

/// Whether the signed-in account may save videos to its device (the `video_download` permission, on unless an
/// admin turned it off; admins always may).
fn may_save_video() -> bool {
    crate::app::is_admin() || crate::app::user().is_some_and(|u| u.g("perms").truthy("video_download"))
}

/// One video download's card in `#video-out` (`id`: the job; `body`: what it says now).
fn video_card(id: &str, body: &str) -> String {
    format!(r#"<div class="card flat row" data-video="{}"><div class="grow">{body}</div><button class="btn sm ghost" type="button" data-video-cancel="{}" title="Stop and delete it from the server">{}</button></div>"#,
        esc(id), esc(id), icon("x"))
}

/// Save video to this device: the server downloads the video at `url` (POST /api/video-links), this page
/// shows its progress in a card added to `out`, and once it is ready the browser saves the MP4
/// (GET …/file as a download), after which the server deletes its copy. The card's ✕ cancels. → whether the
/// download was handed to the browser.
async fn save_video(url: &str, out: &Element) -> bool {
    let job = match api::post("/video-links", json!({ "url": url })).await {
        Ok(j) => j,
        Err(e) => { toast(&e.message, "err"); return false; }
    };
    let id = job.s("id");
    let card = dom::create("div");
    dom::set_html(&card, &video_card(&id, r#"<span class="spinner"></span> Getting the video…"#));
    let _ = out.prepend_with_node_1(&card);
    {
        let (id, card) = (id.clone(), card.clone());
        dom::on_sel(&card.clone(), "[data-video-cancel]", "click", move |_| {
            let (id, card) = (id.clone(), card.clone());
            dom::spawn(async move {
                let _ = api::del(&format!("/video-links/{id}")).await;
                dom::remove(&card);
            });
        });
    }
    let body = |html: &str| if let Some(b) = dom::qs_in(&card, ".grow") { dom::set_html(&b, html); };
    // until the card is gone (cancelled, or the page left), once a second
    while card.is_connected() {
        let j = match api::get(&format!("/video-links/{id}")).await {
            Ok(j) => j,
            Err(e) => { body(&format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); return false; }
        };
        let title = j.s("title");
        match j.s("state").as_str() {
            "ready" => {
                let name = j.s("name");
                // a plain link with `download`: the browser saves it with its own progress, signed in by cookie
                let a = dom::create("a");
                let _ = a.set_attribute("href", &format!("/api/video-links/{id}/file"));
                let _ = a.set_attribute("download", &name);
                dom::click(&a);
                body(&format!(r#"{} <b>{}</b><div class="small muted">{} · saving to this device – the server deletes its copy once it's sent</div>"#,
                    icon("check"), esc(&name), ui::fmt::size(j.f("size"))));
                // nothing left on the server to cancel
                if let Some(x) = dom::qs_in(&card, "[data-video-cancel]") { dom::remove(&x); }
                return true;
            }
            "failed" => {
                body(&format!(r#"<div class="error-box">{}</div>"#, esc(&j.s("error"))));
                return false;
            }
            state => {
                let what = if title.is_empty() { "the video".to_string() } else { format!("<b>{}</b>", esc(&title)) };
                let pct = j.f("progress");
                body(&format!(r#"<span class="spinner"></span> {} {what}<div class="progress mt" style="height:6px"><i style="width:{}%"></i></div>"#,
                    if state == "queued" { "Waiting to download" } else { "Downloading" }, crate::num::js_num(pct.round())));
            }
        }
        dom::sleep(1000).await;
    }
    false
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
        {save_video}
      </div>
      <div id="probe-out"></div>
      <div id="video-out" class="stack"></div>
    </form>
    <div class="section-head"><h2>Recent downloads</h2><div class="row" style="gap:8px">{retry_failed}<button class="btn sm" id="dl-clear">{trash}Clear…</button></div></div>
    <div class="card flat" id="dl-list"></div>"##,
            link = icon("link"), download = icon("download"), list = icon("list"), trash = icon("trash"),
            // the video comes to this device and is deleted from the server: never into the library
            save_video = if may_save_video() {
                format!(r#"<button class="btn ghost" type="button" id="save-video" title="Download the video as an MP4 to this device – it isn't kept on the server or added to the library">{}Save video to this device</button>"#, icon("video"))
            } else { String::new() },
            // members without Fix their own downloads may not retry (the server answers "Ask an admin to retry")
            retry_failed = if common::may_fix() {
                format!(r#"<button class="btn sm" id="dl-retry-failed" title="Queue every failed download again">{}Retry failed</button>"#, icon("refresh"))
            } else { String::new() }));
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

        if let Some(btn) = dom::qs_in(&view, "#save-video") {
            let (u, b) = (url_in.clone(), btn.clone());
            let video_out = need(&view, "#video-out")?;
            dom::on(&btn, "click", move |e| {
                dom::prevent(e);
                let url = dom::value(&u).trim().to_string();
                if url.is_empty() { dom::focus(&u); return; }
                let (b, o, u) = (b.clone(), video_out.clone(), u.clone());
                dom::set_disabled(&b, true);
                dom::spawn(async move {
                    if save_video(&url, &o).await { dom::set_value(&u, ""); }
                    dom::set_disabled(&b, false);
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
            dom::on(&clear, "click", move |_| common::clear_menu(&c, l.clone(), common::Whose::mine()));
        }
        if let Some(retry) = dom::qs_in(&view, "#dl-retry-failed") {
            let l = load.clone();
            dom::on(&retry, "click", move |_| { let l = l.clone(); common::retry_failed(common::Whose::mine(), l); });
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
