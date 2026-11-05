//! The Downloader page: paste a link, get its songs (matched to the best source, or downloaded as they are),
//! import it as a playlist or save a video to this device, with the reader's recent downloads under it.
//! Admins also see the transfers, queue and server log (the Activity page) at the bottom.
//!
//! A Leptos view ([`Downloader`]) over the page's signals. The request rows and the video cards are shared HTML
//! (`common::request_row`, [`video_card`]) and keep their own handlers.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::pages::{activity, common, playlists};
use crate::ui::{self, art, esc, icon, toast};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;

// links that only describe tracks (no downloadable audio) → resolved track-by-track
const METADATA_ONLY: &str = r"open\.spotify\.com|spotify\.link|^spotify:|music\.apple\.com|music\.amazon\.|amazon\.[a-z.]+/music/player/|amzn\.to/|listenbrainz\.org";
const STREAMRIP: &str = r"qobuz\.com|tidal\.com|deezer\.com|deezer\.page\.link";

/// `/re/i.test(s)`.
fn test(re: &str, s: &str) -> bool { js_sys::RegExp::new(re, "i").test(s) }

/// The reader's 30 most recent link, song and album requests.
async fn load_rows() -> Result<Vec<Value>, api::ApiError> {
    Ok(api::get("/requests?mine=true&limit=100").await?.items().into_iter()
        .filter(|r| matches!(r.g("kind").as_str(), Some("url" | "track" | "album"))).take(30).collect())
}

/// The recent downloads as HTML: their rows, or a line saying there are none yet.
fn rows_html(rows: &[Value]) -> String {
    if rows.is_empty() {
        return r#"<div class="empty muted">Nothing yet – paste a link above.</div>"#.to_string();
    }
    format!(r#"<div class="list">{}</div>"#, rows.iter().map(|r| common::request_row(r, false)).collect::<String>())
}

/// What Exactly this link says on hover for the link typed (when it is disabled, why).
fn direct_title(url: &str) -> &'static str {
    if test(METADATA_ONLY, url) {
        "This service has no downloadable audio – use “Import as playlist”"
    } else if test(STREAMRIP, url) {
        "Rip this exact release with streamrip"
    } else {
        "Skip matching and download exactly this link"
    }
}

/// What Download says it did: the songs it requested, or that it is downloading the link as it is.
fn requested_text(r: &Value, album_mode: &str) -> String {
    if r.s("mode") == "direct" {
        return "Downloading".to_string();
    }
    format!("{} song(s) requested{}{}", r.s("requested"),
        if r.truthy("already_requested") { format!(", {} already requested", r.s("already_requested")) } else { String::new() },
        if album_mode == "album" { " – whole albums will be fetched" } else { "" })
}

/// The Preview answer as HTML (the thumbnail, the title and what it is), and the title the request takes.
fn probe_html(p: &Value) -> (String, String) {
    let title = p.s("title");
    let sub = [
        p.s("extractor"),
        p.s("uploader"),
        if p.truthy("is_playlist") { format!("{} items", p.s("count")) } else if p.truthy("duration") { ui::fmt::dur(p.f("duration")) } else { String::new() },
    ].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let shown = if title.is_empty() { "Untitled".to_string() } else { title.clone() };
    let html = format!(r#"<div class="row">
        <div class="thumb lg">{}</div>
        <div class="grow"><b>{}</b>
          <div class="small muted">{}</div>
        </div></div>"#, art(&p.s("thumbnail"), &title, ""), esc(&shown), esc(&sub));
    (html, shown)
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

/// The page's state.
#[derive(Clone, Copy)]
struct Page {
    /// The link typed.
    url: RwSignal<String>,
    /// What to do when a song is on an album: `song`, `album` or "" (the admin's default); kept on this device.
    album_mode: RwSignal<String>,
    /// Download is looking the link up.
    looking: RwSignal<bool>,
    /// Exactly this link is sending its request.
    direct_busy: RwSignal<bool>,
    /// Save video to this device is starting a download.
    video_busy: RwSignal<bool>,
    /// What Preview shows (HTML), and the title it found (the request's title for Exactly this link).
    probe: RwSignal<(String, Option<String>)>,
    /// The recent downloads.
    rows: RwSignal<Vec<Value>>,
}

impl Page {
    /// Loads the recent downloads again (a failure keeps the list as it is).
    fn reload(self) -> Rc<dyn Fn()> {
        Rc::new(move || {
            dom::spawn(async move {
                if let Ok(rows) = load_rows().await {
                    self.rows.try_set(rows);
                }
            })
        })
    }

    /// The link as typed, without the spaces around it.
    fn link(self) -> String {
        self.url.get_untracked().trim().to_string()
    }

    /// Opens Import playlist on the link, set to request its songs with this page's album choice, and previews it.
    fn import(self) {
        let url = self.link();
        if url.is_empty() {
            focus_url();
            return;
        }
        playlists::import_modal(&url, None);
        let mode = self.album_mode.get_untracked();
        dom::timeout(50, move || {
            if let Some(req) = dom::qs("#imp-req") { dom::set_checked(&req, true); }
            if let Some(sel) = dom::qs("#imp-album") { dom::set_value(&sel, &mode); }
            if let Some(p) = dom::qs("#imp-preview") { dom::click(&p); }
        });
    }

    /// Download: the link's songs are requested (or the link downloaded as it is); the box is emptied after.
    fn download(self) {
        let url = self.link();
        if url.is_empty() {
            focus_url();
            return;
        }
        self.looking.set(true);
        dom::spawn(async move {
            let mode = self.album_mode.get_untracked();
            let album_mode = if mode.is_empty() { Value::Null } else { Value::String(mode.clone()) };
            match api::post("/requests/link", json!({ "url": url, "album_mode": album_mode })).await {
                Ok(r) => {
                    toast(&requested_text(&r, &mode), "ok");
                    self.url.try_set(String::new());
                    self.probe.try_set((String::new(), None));
                    dom::dispatch("encore:requests-changed", None);
                    self.reload()();
                }
                Err(err) => toast(&err.message, "err"),
            }
            self.looking.try_set(false);
        });
    }

    /// Preview: what the link holds (sites with no audio, and streamrip's, open Import playlist instead).
    fn preview(self) {
        let url = self.link();
        if url.is_empty() {
            return;
        }
        if test(METADATA_ONLY, &url) || test(STREAMRIP, &url) {
            self.import();
            return;
        }
        self.probe.set((r#"<span class="spinner"></span>"#.to_string(), None));
        dom::spawn(async move {
            let answer = match api::get(&format!("/discover/probe?url={}", String::from(js_sys::encode_uri_component(&url)))).await {
                Ok(p) => {
                    let (html, title) = probe_html(&p);
                    (html, Some(title))
                }
                Err(e) => (format!(r#"<div class="error-box">{}</div>"#, esc(&e.message)), None),
            };
            self.probe.try_set(answer);
        });
    }

    /// Exactly this link: the link is requested as it is, with the title Preview found.
    fn direct(self) {
        let url = self.link();
        if url.is_empty() {
            focus_url();
            return;
        }
        if test(METADATA_ONLY, &url) {
            self.import();
            return;
        }
        self.direct_busy.set(true);
        let title = self.probe.with_untracked(|p| p.1.clone()).map(Value::String).unwrap_or(Value::Null);
        dom::spawn(async move {
            // it toasts a failure itself
            let _ = common::submit_request(json!({ "kind": "url", "url": url, "title": title }), None).await;
            self.url.try_set(String::new());
            self.probe.try_set((String::new(), None));
            self.reload()();
            self.direct_busy.try_set(false);
        });
    }

    /// Save video to this device: a card for the download in `#video-out`; the box is emptied once it is saved.
    fn save_video(self) {
        let url = self.link();
        if url.is_empty() {
            focus_url();
            return;
        }
        let Some(out) = dom::qs("#video-out") else { return };
        self.video_busy.set(true);
        dom::spawn(async move {
            if save_video(&url, &out).await {
                self.url.try_set(String::new());
            }
            self.video_busy.try_set(false);
        });
    }
}

/// Puts the cursor in the link box.
fn focus_url() {
    if let Some(u) = dom::qs("#dl-form input[name=url]") {
        dom::focus(&u);
    }
}

/// The Downloader page: the view, mounted once the recent downloads have come (their failure is the page's
/// error), refreshed every 4 s; the Activity page under it for admins.
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let rows = load_rows().await.map_err(|e| e.message)?;
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let page = owner.with(|| Page {
            url: RwSignal::new(String::new()),
            album_mode: RwSignal::new(dom::ls_get("encore.albumMode").unwrap_or_else(|| "song".into())),
            looking: RwSignal::new(false),
            direct_busy: RwSignal::new(false),
            video_busy: RwSignal::new(false),
            probe: RwSignal::new((String::new(), None)),
            rows: RwSignal::new(rows),
        });
        let (may_video, may_fix) = (may_save_video(), common::may_fix());
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Downloader page may_video may_fix/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        let list = dom::qs_in(&view, "#dl-list").ok_or("#dl-list")?;
        common::bind_request_actions(&list, page.reload());
        // imports paused because a folder can't be written (R1): the same banner as on Requests
        if let Some(slot) = dom::qs_in(&view, "#storage-banner") {
            common::storage_banner(&view, &slot, admin);
        }
        let reload = page.reload();
        dom::interval(&view, 4000, move || reload());
        if admin {
            // transfers, torrent/Usenet queue and the server log live here too (was the Activity page)
            let act = dom::create("div");
            act.set_class_name("embedded mt-lg");
            let _ = view.append_child(&act);
            // not waited for: the page is shown without it
            dom::spawn(async move { let _ = activity::render(act, ctx).await; });
        }
        Ok(())
    })
}

/// The modes of "If a song is on an album" (`data-m`) and their labels.
const ALBUM_MODES: &[(&str, &str)] = &[("song", "Just the song"), ("album", "Get the whole album"), ("", "Admin default")];

/// The page: the title and what links work, the form with its buttons, Preview's answer and the video cards,
/// and the recent downloads.
#[component]
fn Downloader(
    /// The page's state.
    page: Page,
    /// Whether the reader may save videos to their device.
    may_video: bool,
    /// Whether the reader may retry their own failed downloads.
    may_fix: bool,
) -> impl IntoView {
    let link = icon("link");
    let modes = ALBUM_MODES
        .iter()
        .map(|&(m, l)| {
            let pick = move |_| {
                page.album_mode.set(m.to_string());
                dom::ls_set("encore.albumMode", m);
            };
            view! { <button type="button" data-m=m class:active=move || page.album_mode.get() == m on:click=pick>{l}</button> }
        })
        .collect_view();
    let on_submit = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        page.download();
    };
    let get_songs = move || {
        if page.looking.get() { r#"<span class="spinner"></span> Looking up…"#.to_string() } else { format!("{}Download", icon("download")) }
    };
    let blocked = move || test(METADATA_ONLY, page.url.get().trim());
    // the video comes to this device and is deleted from the server: never into the library
    let save_video = may_video.then(|| view! {
        <button class="btn ghost" type="button" id="save-video" title="Download the video as an MP4 to this device – it isn't kept on the server or added to the library"
            disabled=move || page.video_busy.get() on:click=move |_| page.save_video() {..html(format!("{}Save video to this device", icon("video")))}>{""}</button>
    });
    // members without Fix their own downloads may not retry (the server answers "Ask an admin to retry")
    let retry_failed = may_fix.then(|| view! {
        <button class="btn sm" id="dl-retry-failed" title="Queue every failed download again" on:click=move |_| common::retry_failed(common::Whose::mine(), page.reload())
            {..html(format!("{}Retry failed", icon("refresh")))}>{""}</button>
    });
    let on_clear = move |e: web_sys::MouseEvent| {
        if let Some(b) = dom::target_el(&e).and_then(|t| dom::closest_of(&t, "#dl-clear")) {
            common::clear_menu(&b, page.reload(), common::Whose::mine());
        }
    };
    view! {
        <h1>"Downloader"</h1>
        <p class="muted" style="margin:6px 0 20px">"Paste a link from anywhere. "<b>"Spotify, Apple Music, YouTube Music, Deezer, Tidal, Qobuz, Amazon Music"</b>" songs, albums and playlists are matched song by song (exact ISRC when available) and fetched from the best source you have – lossless first. "
            <b>"YouTube, SoundCloud, Bandcamp, Mixcloud"</b>" and ~1,800 other sites download directly. Every file is fingerprint-verified and de-duplicated before it hits the library."</p>
        <div id="storage-banner"></div>
        <form class="card stack" id="dl-form" on:submit=on_submit>
            <div class="row wrap">
                <div class="search grow">{raw(link.clone())}<input class="input" name="url" placeholder="https://…" required autocomplete="off"
                    prop:value=move || page.url.get() on:input=move |e| page.url.set(event_target_value(&e))/></div>
                <button class="btn" type="button" id="probe" on:click=move |_| page.preview()>"Preview"</button>
            </div>
            <div class="row wrap">
                <span class="small muted">"If a song is on an album:"</span>
                <div class="tabs" id="album-mode">{modes}</div>
            </div>
            <div class="row wrap" id="dl-actions">
                <button class="btn primary" type="submit" id="get-songs" title="Works with any link: matched to the best source, or downloaded directly"
                    disabled=move || page.looking.get() {..html(get_songs)}>{""}</button>
                <button class="btn" type="button" id="as-playlist" on:click=move |_| page.import() {..html(format!("{}Import as playlist", icon("list")))}>{""}</button>
                <button class="btn ghost" type="button" id="direct" title=move || direct_title(page.url.get().trim())
                    disabled=move || blocked() || page.direct_busy.get() on:click=move |_| page.direct() {..html(format!("{link}Exactly this link"))}>{""}</button>
                {save_video}
            </div>
            <div id="probe-out" inner_html=move || page.probe.with(|p| p.0.clone())></div>
            <div id="video-out" class="stack"></div>
        </form>
        <div class="section-head"><h2>"Recent downloads"</h2><div class="row" style="gap:8px">{retry_failed}
            <button class="btn sm" id="dl-clear" on:click={on_clear} {..html(format!("{}Clear…", icon("trash")))}>{""}</button></div></div>
        <div class="card flat" id="dl-list" inner_html=move || page.rows.with(|r| rows_html(r))></div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Download says how many songs it asked for, and that whole albums come when that was chosen.
    #[test]
    fn download_says_what_it_requested() {
        assert_eq!(requested_text(&json!({ "mode": "direct" }), "album"), "Downloading");
        assert_eq!(requested_text(&json!({ "requested": 4 }), "song"), "4 song(s) requested");
        assert_eq!(requested_text(&json!({ "requested": 4, "already_requested": 2 }), "album"), "4 song(s) requested, 2 already requested – whole albums will be fetched");
    }
}
