//! The Downloads page (#/downloads, 0.28): the songs saved in this browser to play without the server, grouped
//! by album, with the room they take, a search box, play buttons and ⋯ menus. Also the "Download for offline" /
//! "Remove download" entries the other menus add (song, album, playlist, the player) and the download itself,
//! which runs in the background with a progress toast.
//!
//! All of it only where this browser can keep downloads (`offline::available()`: https or localhost).

use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::J;
use crate::offline::{self, mode, outbox, Progress, Record, Summary};
use crate::ui::{self, confirm_dialog, empty_state, esc, fmt, icon, pop_menu, toast, MenuItem};
use crate::player;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use web_sys::Element;

pub const EMPTY_TITLE: &str = "Nothing downloaded yet";
pub const EMPTY_TEXT: &str = "Use ⋯ → Download for offline on a song, an album or a playlist.";
/// On an iPhone or iPad: Safari clears a website's storage after 7 days of use without a visit, while a Home
/// Screen app is exempt but keeps its own storage, apart from Safari's (downloads made in a tab don't follow).
pub const IOS_HINT: &str = "Add Encore to your Home Screen (Share → Add to Home Screen) and download from there: Safari clears a website's downloads after a week without a visit, and the Home Screen app keeps its own.";
/// localStorage: the iOS hint was shown with a first download in this browser.
const IOS_HINT_SHOWN: &str = "encore.offline.iosHint";
pub const PERSIST_REFUSED: &str = "The browser said no for now. Browsers usually allow it for sites you use often or add to the Home Screen; until then the downloads may be cleared when the device runs low on space.";
pub const SIGN_IN_AGAIN: &str = "Sign in again when the server is back to keep playing downloaded songs";
pub const STILL_CONVERTED: &str = "This browser can't play the original file, so the converted copy stays";
pub const STILL_CONVERTED_ALL: &str = "This browser can't play the original files, so the converted copies stay";

/// Web downloads apply here: a browser that can keep them.
pub fn enabled() -> bool { offline::available() }

// ---- the menu entries and the download ------------------------------------------------------------

/// Which entries a menu shows for `ids` songs of which `saved` are downloaded: (Download for offline,
/// Remove download). Offline there is nothing to download from.
pub fn entries(ids: usize, saved: usize, offline_mode: bool) -> (bool, bool) {
    (ids > 0 && saved < ids && !offline_mode, saved > 0)
}

/// The library ids among these rows, once each.
fn ids_of(rows: &[Value]) -> Vec<i64> {
    let mut out: Vec<i64> = vec![];
    for id in rows.iter().filter_map(offline::download::track_id) {
        if !out.contains(&id) { out.push(id); }
    }
    out
}

/// "Download for offline" (some of these songs are not saved yet) and "Remove download" (some are) for a song,
/// an album or a playlist menu. Empty when web downloads don't apply here.
pub fn menu_items(rows: &[Value]) -> Vec<Option<MenuItem>> {
    let mut out = vec![];
    if !enabled() { return out; }
    let ids = ids_of(rows);
    let saved: Vec<i64> = ids.iter().copied().filter(|id| offline::has(*id)).collect();
    let (download, remove) = entries(ids.len(), saved.len(), mode::active());
    if download {
        let rows = rows.to_vec();
        out.push(MenuItem::new("Download for offline", &icon("download"), move || start(rows.clone())));
    }
    if remove {
        out.push(MenuItem::new("Remove download", &icon("trash"), move || remove_songs(saved.clone())));
    }
    out
}

/// "Downloading 3 of 12 · <title>".
pub fn progress_text(p: &Progress) -> String {
    let n = (p.done + 1).min(p.total.max(1));
    if p.current.trim().is_empty() { format!("Downloading {n} of {}", p.total) } else { format!("Downloading {n} of {} · {}", p.total, p.current.trim()) }
}

fn songs(n: usize) -> String { if n == 1 { "1 song".into() } else { format!("{n} songs") } }

/// The toast at the end: "12 songs downloaded", "10 downloaded, 2 failed – <why>" and its kind.
pub fn summary_text(s: &Summary) -> (String, &'static str) {
    let why = s.failed.first().map(|(_, r)| format!(" – {r}")).unwrap_or_default();
    match (s.saved, s.failed.len()) {
        (0, 0) => (if s.skipped > 0 { "Already downloaded".into() } else { "Nothing to download".into() }, "ok"),
        (n, 0) => (format!("{} downloaded", songs(n)), "ok"),
        (0, f) => (format!("{} couldn't be downloaded{why}", songs(f)), "err"),
        (n, f) => (format!("{n} downloaded, {f} failed{why}"), "warn"),
    }
}

/// A toast that stays and changes its text: the progress of a download.
#[derive(Clone, Default)]
struct ProgressToast(Rc<RefCell<Option<Element>>>);
impl ProgressToast {
    fn set(&self, text: &str) {
        let mut el = self.0.borrow_mut();
        if el.is_none() {
            let Some(host) = dom::by_id("toasts") else { return };
            let t = dom::create("div");
            t.set_class_name("toast dl-toast");
            let _ = t.set_attribute("role", "status");
            let _ = host.append_child(&t);
            *el = Some(t);
        }
        if let Some(t) = el.as_ref() { dom::set_text(t, text); }
    }
    fn close(&self) { if let Some(t) = self.0.borrow_mut().take() { t.remove(); } }
}

/// Save these library rows on this device in the background: a progress toast while it runs, then how it went.
pub fn start(rows: Vec<Value>) {
    ios_hint_once();
    dom::spawn(async move {
        let t = ProgressToast::default();
        let t2 = t.clone();
        let s = offline::download(&rows, move |p| t2.set(&progress_text(&p))).await;
        t.close();
        let (text, kind) = summary_text(&s);
        toast(&text, kind);
    });
}

/// In Safari on an iPhone or iPad, the first download in this browser says where downloads last, before the
/// songs are saved in the tab (the Downloads page says it too, but only once the downloading is done).
fn ios_hint_once() {
    if dom::ls_get(IOS_HINT_SHOWN).is_some() || !ios_hint_here() { return; }
    dom::ls_set(IOS_HINT_SHOWN, "1");
    let t = ProgressToast::default();
    t.set(IOS_HINT);
    gloo_timers::callback::Timeout::new(12_000, move || t.close()).forget();
}

/// Delete these downloads.
pub fn remove_songs(ids: Vec<i64>) {
    dom::spawn(async move {
        match offline::remove(&ids).await {
            Ok(()) => toast(&if ids.len() == 1 { "Download removed".to_string() } else { format!("{} downloads removed", ids.len()) }, "ok"),
            Err(e) => toast(&e, "err"),
        }
    });
}

/// A saved song as the player's queue wants it. A converted copy leaves its format out: the server's file is
/// not that codec, and the player would ask for the wrong stream when it plays from the server.
pub fn track_of(r: &Record) -> Value {
    let mut t = json!({ "id": r.id, "title": r.title, "artist": r.artist, "album": r.album, "duration": r.duration, "track": r.track, "disc": r.disc });
    if !r.album_id.is_empty() { t["album_id"] = json!(r.album_id); }
    if !r.artists.is_empty() { t["artists"] = json!(r.artists); }
    if !r.converted() && !r.ext.is_empty() && r.ext != "audio" { t["format"] = json!(r.ext); }
    t
}

/// The row "Download again" sends: the song without its saved format, so the server's `/info` decides.
fn again_row(r: &Record) -> Value {
    let mut t = track_of(r);
    if let Some(o) = t.as_object_mut() { o.remove("format"); }
    t
}

/// Online: the server can be asked (not offline mode, and the browser thinks it has a network).
fn online() -> bool { !mode::active() && dom::window().navigator().on_line() }

/// "Download again in original quality" for these converted copies.
fn download_again(recs: Vec<Record>) {
    dom::spawn(async move {
        let rows: Vec<Value> = recs.iter().map(again_row).collect();
        let t = ProgressToast::default();
        let t2 = t.clone();
        let s = offline::download_again(&rows, move |p| t2.set(&progress_text(&p))).await;
        t.close();
        if let Some((_, why)) = s.failed.first() { toast(why, "err"); return; }
        let still = recs.iter().filter(|r| offline::record(r.id).is_some_and(|x| x.converted())).count();
        let (text, kind) = again_text(recs.len(), still);
        toast(&text, kind);
    });
}

/// The toast after "Download again in original quality" for `n` songs of which `still` stay converted (the
/// original still can't be saved here, so nothing was fetched for them).
pub fn again_text(n: usize, still: usize) -> (String, &'static str) {
    match (n.saturating_sub(still), still) {
        (_, 0) if n == 1 => ("Downloaded again in original quality".into(), "ok"),
        (_, 0) => (format!("{} downloaded again in original quality", songs(n)), "ok"),
        (0, 1) => (STILL_CONVERTED.into(), "warn"),
        (0, _) => (STILL_CONVERTED_ALL.into(), "warn"),
        (done, still) => (format!("{} downloaded again in original quality; {still} {} converted because this browser can't play the original",
            songs(done), if still == 1 { "stays" } else { "stay" }), "warn"),
    }
}

// ---- the page: pure parts ---------------------------------------------------------------------------

/// One album on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub album: String,
    pub album_id: String,
    pub artist: String,
    pub songs: Vec<Record>,
}
impl Group {
    pub fn bytes(&self) -> u64 { self.songs.iter().map(|r| r.bytes).sum() }
    fn latest(&self) -> i64 { self.songs.iter().map(|r| r.saved_at).max().unwrap_or(0) }
}

/// Every word of the search appears in the song's title, artist or album.
pub fn matches(r: &Record, query: &str) -> bool {
    let hay = format!("{} {} {}", r.title, r.artist, r.album).to_lowercase();
    query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// The artist an album shows: the one most of its songs have ("Various artists" when none stands out).
pub fn album_artist(songs: &[Record]) -> String {
    let mut n: Vec<(String, usize)> = vec![];
    for r in songs.iter().filter(|r| !r.artist.trim().is_empty()) {
        match n.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(&r.artist)) { Some(x) => x.1 += 1, None => n.push((r.artist.clone(), 1)) }
    }
    match n.len() {
        0 => String::new(),
        1 => n.remove(0).0,
        _ => {
            let best = n.iter().map(|x| x.1).max().unwrap_or(0);
            if best * 2 > songs.len() { n.into_iter().find(|x| x.1 == best).map(|x| x.0).unwrap_or_default() } else { "Various artists".into() }
        }
    }
}

/// The saved songs that match `query`, grouped by album (newest download first), each album in disc and
/// track order.
pub fn group(records: &[Record], query: &str) -> Vec<Group> {
    let mut order: Vec<String> = vec![];
    let mut by: HashMap<String, Vec<Record>> = HashMap::new();
    for r in records.iter().filter(|r| matches(r, query)) {
        let key = if r.album_id.is_empty() { format!("name:{}", r.album.trim().to_lowercase()) } else { format!("id:{}", r.album_id) };
        if !by.contains_key(&key) { order.push(key.clone()); }
        by.entry(key).or_default().push(r.clone());
    }
    let mut out: Vec<Group> = order.into_iter().filter_map(|k| by.remove(&k)).map(|mut songs| {
        songs.sort_by_key(|a| (a.disc, a.track, a.title.to_lowercase()));
        let first = &songs[0];
        Group { album: first.album.clone(), album_id: first.album_id.clone(), artist: album_artist(&songs), songs }
    }).collect();
    out.sort_by(|a, b| b.latest().cmp(&a.latest()).then_with(|| a.album.to_lowercase().cmp(&b.album.to_lowercase())));
    out
}

/// "1.2 GB of 50.0 GB used · 34 songs · persistent".
pub fn storage_line(estimate: Option<(f64, f64)>, saved_bytes: f64, n: usize, persisted: Option<bool>) -> String {
    let mut parts = vec![];
    match estimate {
        Some((used, quota)) if quota > 0.0 => parts.push(format!("{} of {} used", fmt::size(used), fmt::size(quota))),
        _ if saved_bytes > 0.0 => parts.push(fmt::size(saved_bytes)),
        _ => {}
    }
    parts.push(songs(n));
    match persisted {
        Some(true) => parts.push("persistent".into()),
        Some(false) => parts.push("not persistent".into()),
        None => {}
    }
    parts.join(" · ")
}

/// An iPhone or iPad (iPadOS says it is a Mac with a touch screen) in Safari rather than from the home screen:
/// Safari may clear what a site stored after a week without a visit, unless it was added to the home screen.
pub fn ios_hint(user_agent: &str, platform: &str, touch_points: f64, standalone: bool) -> bool {
    let ios = ["iPhone", "iPad", "iPod"].iter().any(|d| user_agent.contains(d)) || (platform == "MacIntel" && touch_points > 1.0);
    ios && !standalone
}

fn ios_hint_here() -> bool {
    let nav = dom::window().navigator();
    let get = |k: &str| js_sys::Reflect::get(&nav, &k.into()).ok();
    ios_hint(
        &nav.user_agent().unwrap_or_default(),
        &nav.platform().unwrap_or_default(),
        get("maxTouchPoints").and_then(|v| v.as_f64()).unwrap_or(0.0),
        get("standalone").and_then(|v| v.as_bool()).unwrap_or(false),
    )
}

// ---- the page --------------------------------------------------------------------------------------

struct Page {
    view: Element,
    list: Element,
    records: RefCell<Vec<Record>>,
    groups: RefCell<Vec<Group>>,
    query: RefCell<String>,
}

fn group_html(gi: usize, g: &Group, playable: bool) -> String {
    let rows: String = g.songs.iter().enumerate().map(|(i, r)| {
        let artist = if r.artist.is_empty() || r.artist == g.artist { String::new() } else { format!(r#"<div class="small muted ellipsis">{}</div>"#, esc(&r.artist)) };
        format!(r#"<tr data-i="{i}" data-id="{id}">
        <td class="num"><span class="n">{n}</span><span class="rowplay" aria-hidden="true">{play}</span></td>
        <td><div class="ellipsis" style="font-weight:600">{title}</div>{artist}</td>
        <td class="hide-sm"><span class="badge{conv}">{quality}</span></td>
        <td class="hide-sm muted">{size}</td>
        <td class="muted">{dur}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost icon" data-dl-menu aria-label="More" title="More">{more}</button></td></tr>"#,
            id = r.id, n = if r.track > 0 { r.track as usize } else { i + 1 }, play = icon("play"), title = esc(if r.title.is_empty() { "Unknown song" } else { &r.title }),
            conv = if r.converted() { " warn" } else { "" }, quality = esc(if r.quality.is_empty() { "Audio" } else { &r.quality }),
            size = if r.bytes > 0 { fmt::size(r.bytes as f64) } else { String::new() },
            dur = if r.duration > 0.0 { fmt::dur(r.duration) } else { String::new() }, more = icon("more"))
    }).collect();
    let first = g.songs.first().map(|r| r.id).unwrap_or(0);
    let album = if g.album.is_empty() { "Unknown album".to_string() } else { g.album.clone() };
    let meta: Vec<String> = [esc(&g.artist), songs(g.songs.len()), fmt::size(g.bytes() as f64)].into_iter().filter(|x| !x.is_empty()).collect();
    format!(r#"<section class="dl-album card flat" data-g="{gi}">
    <div class="dl-album-head">
      <div class="thumb lg" data-dl-cover="{first}">{ph}</div>
      <div class="grow" style="min-width:0">
        <div class="dl-album-title ellipsis" title="{album}">{album}</div>
        <div class="small muted ellipsis">{meta}</div>
      </div>
      <button class="btn sm primary icon{off}" data-dl-play-album aria-label="Play" title="{play_title}"{dis}>{play}</button>
      <button class="btn sm ghost icon" data-dl-album-menu aria-label="More" title="More">{more}</button>
    </div>
    <div class="table-wrap"><table class="table tracklist dl-songs"><tbody>{rows}</tbody></table></div>
  </section>"#,
        ph = ui::art("", &album, ""), album = esc(&album), meta = meta.join(" · "),
        off = if playable { "" } else { " offline-disabled" }, dis = if playable { "" } else { " disabled" },
        play_title = if playable { "Play" } else { mode::DISABLED_TITLE },
        play = icon("play"), more = icon("more"))
}

impl Page {
    fn records_shown(&self) -> Vec<Record> { self.groups.borrow().iter().flat_map(|g| g.songs.clone()).collect() }

    fn paint_list(&self) {
        let recs = self.records.borrow().clone();
        let q = self.query.borrow().clone();
        let groups = group(&recs, &q);
        let playable = mode::can_play();
        for id in ["dl-play", "dl-shuffle", "dl-remove-all"] {
            if let Some(b) = dom::qs_in(&self.view, &format!("#{id}")) { dom::hide(&b, recs.is_empty()); }
        }
        if let Some(s) = dom::qs_in(&self.view, "#dl-search") { dom::hide(&s, recs.len() < 2); }
        let html = if recs.is_empty() {
            empty_state(&icon("saved"), EMPTY_TITLE, &esc(EMPTY_TEXT))
        } else if groups.is_empty() {
            format!(r#"<p class="muted">No downloads match “{}”.</p>"#, esc(q.trim()))
        } else {
            groups.iter().enumerate().map(|(i, g)| group_html(i, g, playable)).collect()
        };
        set_html(&self.list, &html);
        *self.groups.borrow_mut() = groups;
        self.load_covers();
        self.highlight(player::current());
    }

    /// The saved covers (object URLs the store keeps per album); online, the server's when none was saved.
    fn load_covers(&self) {
        let groups = self.groups.borrow().clone();
        for el in dom::qsa_in(&self.list, "[data-dl-cover]") {
            let Some(g) = dom::closest_of(&el, "[data-g]").and_then(|s| dom::data(&s, "g").parse::<usize>().ok()).and_then(|i| groups.get(i).cloned()) else { continue };
            let id: i64 = dom::data(&el, "dlCover").parse().unwrap_or(0);
            dom::spawn(async move {
                let label = if g.album.is_empty() { "Unknown album".to_string() } else { g.album.clone() };
                if let Some(u) = offline::cover_url(id).await {
                    set_html(&el, &ui::art(&u, &label, ""));
                } else if !mode::active() && !g.album_id.is_empty() {
                    set_html(&el, &ui::art(&format!("/api/library/albums/{}/cover", g.album_id), &label, ""));
                }
            });
        }
    }

    fn highlight(&self, cur: Option<Value>) {
        let id = cur.and_then(|c| c.g("id").as_i64());
        for tr in dom::qsa_in(&self.list, "tr[data-id]") {
            dom::toggle_class(&tr, "playing", id.is_some_and(|id| dom::data(&tr, "id") == id.to_string()));
        }
    }

    fn paint_outbox(&self) {
        let Some(el) = dom::qs_in(&self.view, "#dl-outbox") else { return };
        let (plays, likes) = outbox::pending_counts();
        let text = mode::counts_text(plays, likes);
        dom::hide(&el, text.is_empty());
        if !text.is_empty() { set_html(&el, &format!("{}<span>{} will be sent when the server is back</span>", icon("refresh"), esc(&text))); }
    }

    async fn paint_storage(self: Rc<Self>) {
        let (est, pers) = futures::join!(offline::estimate(), offline::persisted());
        let recs = self.records.borrow().clone();
        let saved: f64 = recs.iter().map(|r| r.bytes as f64).sum();
        if let Some(el) = dom::qs_in(&self.view, "#dl-store") { dom::set_text(&el, &storage_line(est, saved, recs.len(), pers)); }
        if let Some(bar) = dom::qs_in(&self.view, "#dl-bar") {
            let pct = match est { Some((u, q)) if q > 0.0 => (u / q * 100.0).clamp(0.0, 100.0), _ => 0.0 };
            // a sliver stays visible once something is saved
            let pct = if pct > 0.0 { pct.max(0.5) } else { 0.0 };
            dom::set_style(&bar, "width", &format!("{pct:.2}%"));
            if let Some(wrap) = dom::closest_of(&bar, ".progress") { dom::hide(&wrap, est.is_none()); }
        }
        if let Some(b) = dom::qs_in(&self.view, "#dl-persist") { dom::hide(&b, pers != Some(false)); }
    }

    fn play(&self, songs: &[Record], start: usize, context: &str) {
        if !mode::can_play() { toast(SIGN_IN_AGAIN, "warn"); return; }
        if songs.is_empty() { return; }
        player::play_queue(songs.iter().map(track_of).collect(), start, context);
    }

    fn song_menu(self: &Rc<Self>, anchor: &Element, gi: usize, si: usize) {
        let Some(g) = self.groups.borrow().get(gi).cloned() else { return };
        let Some(r) = g.songs.get(si).cloned() else { return };
        let mut items: Vec<Option<MenuItem>> = vec![];
        if mode::can_play() {
            { let (p, songs) = (self.clone(), g.songs); items.push(MenuItem::new("Play", &icon("play"), move || p.play(&songs, si, "album"))); }
            { let t = track_of(&r); items.push(MenuItem::new("Play next", &icon("next"), move || { player::play_next(vec![t.clone()]); toast("Plays next", "ok"); })); }
            { let t = track_of(&r); items.push(MenuItem::new("Add to queue", &icon("list"), move || { player::enqueue(vec![t.clone()]); toast("Added to queue", "ok"); })); }
        }
        if r.converted() && online() {
            let r2 = r.clone();
            items.push(MenuItem::new("Download again in original quality", &icon("download"), move || download_again(vec![r2.clone()])));
        }
        if !items.is_empty() { items.push(MenuItem::SEP); }
        let id = r.id;
        items.push(MenuItem::danger("Remove download", &icon("trash"), move || remove_songs(vec![id])));
        pop_menu(anchor, items);
    }

    fn album_menu(self: &Rc<Self>, anchor: &Element, gi: usize) {
        let Some(g) = self.groups.borrow().get(gi).cloned() else { return };
        let mut items: Vec<Option<MenuItem>> = vec![];
        if mode::can_play() {
            { let (p, songs) = (self.clone(), g.songs.clone()); items.push(MenuItem::new("Play", &icon("play"), move || p.play(&songs, 0, "album"))); }
            { let q: Vec<Value> = g.songs.iter().map(track_of).collect(); items.push(MenuItem::new("Play next", &icon("next"), move || { player::play_next(q.clone()); toast("Plays next", "ok"); })); }
            { let q: Vec<Value> = g.songs.iter().map(track_of).collect(); items.push(MenuItem::new("Add to queue", &icon("list"), move || { player::enqueue(q.clone()); toast("Added to queue", "ok"); })); }
        }
        let converted: Vec<Record> = g.songs.iter().filter(|r| r.converted()).cloned().collect();
        if !converted.is_empty() && online() {
            items.push(MenuItem::new("Download again in original quality", &icon("download"), move || download_again(converted.clone())));
        }
        if !items.is_empty() { items.push(MenuItem::SEP); }
        let ids: Vec<i64> = g.songs.iter().map(|r| r.id).collect();
        items.push(MenuItem::danger("Remove download", &icon("trash"), move || remove_songs(ids.clone())));
        pop_menu(anchor, items);
    }

    /// The saved songs changed (a download finished, a removal, another tab): read them again and redraw.
    async fn refresh(self: Rc<Self>) {
        let recs = offline::index().await;
        if !self.view.is_connected() { return; }
        let changed = *self.records.borrow() != recs;
        if changed {
            *self.records.borrow_mut() = recs;
            self.paint_list();
            self.clone().paint_storage().await;
        }
        self.paint_outbox();
    }
}

fn shuffled(mut v: Vec<Record>) -> Vec<Record> {
    for i in (1..v.len()).rev() {
        let j = (js_sys::Math::random() * (i as f64 + 1.0)).floor() as usize;
        v.swap(i, j.min(i));
    }
    v
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        if !enabled() {
            set_html(&view, &format!("<h1>Downloads</h1>{}", empty_state(&icon("saved"), "Downloads aren't available here", &esc(offline::store::NO_STORE))));
            return Ok(());
        }
        set_html(&view, &format!(r#"
    <div class="dl-head">
      <h1>Downloads</h1>
      <div class="row wrap">
        <button class="btn primary" id="dl-play" hidden>{play}Play</button>
        <button class="btn" id="dl-shuffle" hidden>{shuffle}Shuffle</button>
        <button class="btn danger ghost" id="dl-remove-all" hidden>{trash}Remove all</button>
      </div>
    </div>
    <div class="card dl-storage">
      <div class="row between wrap" style="gap:8px"><span class="small" id="dl-store">…</span><button class="btn sm" id="dl-persist" hidden>Make persistent</button></div>
      <div class="progress dl-bar" hidden><i id="dl-bar" style="width:0%"></i></div>
      <div class="small muted" id="dl-persist-help" hidden>{refused}</div>
    </div>
    <div class="dl-note" id="dl-ios" hidden>{phone}<span>{ios}</span></div>
    <div class="dl-note" id="dl-outbox" hidden></div>
    <div class="search dl-search" id="dl-search" hidden>{search}<input class="input" id="dl-q" type="search" placeholder="Search downloads" autocomplete="off" aria-label="Search downloads"></div>
    <div id="dl-list"><div class="skeleton" style="height:120px"></div></div>"#,
            play = icon("play"), shuffle = icon("shuffle"), trash = icon("trash"), refused = esc(PERSIST_REFUSED),
            phone = icon("download"), ios = esc(IOS_HINT), search = icon("search")));
        let list = dom::qs_in(&view, "#dl-list").ok_or("Downloads: no list")?;
        let records = offline::index().await;
        if !view.is_connected() { return Ok(()); }
        let page = Rc::new(Page { view: view.clone(), list: list.clone(), records: RefCell::new(records), groups: RefCell::new(vec![]), query: RefCell::new(String::new()) });
        page.paint_list();
        page.paint_outbox();
        dom::spawn(page.clone().paint_storage());
        if let Some(el) = dom::qs_in(&view, "#dl-ios") { dom::hide(&el, !ios_hint_here()); }
        if !mode::can_play() {
            for id in ["#dl-play", "#dl-shuffle"] { if let Some(b) = dom::qs_in(&view, id) { mode::mark_disabled(&b); } }
        }

        // the saved songs or the outbox changed
        { let p = page.clone(); ctx.on_leave(dom::listener(&dom::window(), offline::CHANGED_EVENT, move |_| dom::spawn(p.clone().refresh()))); }
        { let p = page.clone(); ctx.on_leave(player::on_change(move |cur| p.highlight(cur))); }
        if let Some(q) = dom::qs_in(&view, "#dl-q") {
            let (p, q2) = (page.clone(), q.clone());
            ctx.on_leave(dom::listener(&q, "input", move |_| { *p.query.borrow_mut() = dom::value(&q2); p.paint_list(); }));
        }
        if let Some(b) = dom::qs_in(&view, "#dl-play") {
            let p = page.clone();
            ctx.on_leave(dom::listener(&b, "click", move |_| p.play(&p.records_shown(), 0, "list")));
        }
        if let Some(b) = dom::qs_in(&view, "#dl-shuffle") {
            let p = page.clone();
            ctx.on_leave(dom::listener(&b, "click", move |_| p.play(&shuffled(p.records_shown()), 0, "list")));
        }
        if let Some(b) = dom::qs_in(&view, "#dl-remove-all") {
            let p = page.clone();
            ctx.on_leave(dom::listener(&b, "click", move |_| {
                let n = p.records.borrow().len();
                dom::spawn(async move {
                    if !confirm_dialog("Remove all downloads", &format!("Remove the {} saved on this device? They stay in the library on the server.", songs(n)), "Remove all", true).await { return; }
                    match offline::remove_all().await {
                        Ok(()) => toast("All downloads removed", "ok"),
                        Err(e) => toast(&e, "err"),
                    }
                });
            }));
        }
        if let Some(b) = dom::qs_in(&view, "#dl-persist") {
            let p = page.clone();
            ctx.on_leave(dom::listener(&b, "click", move |_| {
                let p = p.clone();
                dom::spawn(async move {
                    let ok = offline::persist().await;
                    if let Some(h) = dom::qs_in(&p.view, "#dl-persist-help") { dom::hide(&h, ok); }
                    if ok { toast("Downloads are now kept even when space runs low", "ok"); }
                    p.paint_storage().await;
                });
            }));
        }
        {
            let p = page;
            ctx.on_leave(dom::listener(&list, "click", move |e| {
                let Some(sec) = dom::closest(e, "[data-g]") else { return };
                let Ok(gi) = dom::data(&sec, "g").parse::<usize>() else { return };
                if let Some(b) = dom::closest(e, "[data-dl-album-menu]") { p.album_menu(&b, gi); return; }
                if dom::closest(e, "[data-dl-play-album]").is_some() {
                    let songs = p.groups.borrow().get(gi).map(|g| g.songs.clone()).unwrap_or_default();
                    p.play(&songs, 0, "album");
                    return;
                }
                let Some(tr) = dom::closest(e, "tr[data-i]") else { return };
                let Ok(si) = dom::data(&tr, "i").parse::<usize>() else { return };
                if let Some(b) = dom::closest(e, "[data-dl-menu]") { p.song_menu(&b, gi, si); return; }
                if dom::closest(e, "a, button").is_some() { return; }
                let songs = p.groups.borrow().get(gi).map(|g| g.songs.clone()).unwrap_or_default();
                p.play(&songs, si, "album");
            }));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: i64, album: &str, album_id: &str, artist: &str, track: i64, saved_at: i64) -> Record {
        Record { id, title: format!("Song {id}"), artist: artist.into(), album: album.into(), album_id: album_id.into(), ext: "flac".into(), bytes: 1000,
            quality: "FLAC 16/44.1".into(), saved_at, track, ..Default::default() }
    }

    #[test]
    fn the_menu_offers_what_applies() {
        assert_eq!(entries(1, 0, false), (true, false), "a song not saved");
        assert_eq!(entries(1, 1, false), (false, true), "saved: Remove download");
        assert_eq!(entries(12, 5, false), (true, true), "half an album: both");
        assert_eq!(entries(12, 5, true), (false, true), "offline: nothing to download from");
        assert_eq!(entries(0, 0, false), (false, false), "no library song");
    }

    #[test]
    fn the_toasts_say_how_far_and_how_it_went() {
        assert_eq!(progress_text(&Progress { done: 2, total: 12, bytes: 0.0, current: "Hey Jude".into() }), "Downloading 3 of 12 · Hey Jude");
        assert_eq!(progress_text(&Progress { done: 12, total: 12, bytes: 0.0, current: String::new() }), "Downloading 12 of 12");
        let s = |saved, skipped, failed: Vec<(i64, String)>| Summary { saved, skipped, failed };
        assert_eq!(summary_text(&s(12, 0, vec![])), ("12 songs downloaded".into(), "ok"));
        assert_eq!(summary_text(&s(1, 0, vec![])), ("1 song downloaded".into(), "ok"));
        assert_eq!(summary_text(&s(10, 0, vec![(1, "No room".into()), (2, "No room".into())])), ("10 downloaded, 2 failed – No room".into(), "warn"));
        assert_eq!(summary_text(&s(0, 0, vec![(1, "No room".into())])), ("1 song couldn't be downloaded – No room".into(), "err"));
        assert_eq!(summary_text(&s(0, 3, vec![])), ("Already downloaded".into(), "ok"));
        assert_eq!(again_text(1, 0), ("Downloaded again in original quality".into(), "ok"));
        assert_eq!(again_text(3, 0), ("3 songs downloaded again in original quality".into(), "ok"));
        assert_eq!(again_text(1, 1), (STILL_CONVERTED.into(), "warn"));
        assert_eq!(again_text(4, 4), (STILL_CONVERTED_ALL.into(), "warn"));
        assert_eq!(again_text(3, 1).0, "2 songs downloaded again in original quality; 1 stays converted because this browser can't play the original");
    }

    #[test]
    fn songs_are_grouped_by_album_newest_first_in_track_order() {
        let recs = vec![
            rec(1, "Abbey Road", "a1", "The Beatles", 2, 100),
            rec(2, "Abbey Road", "a1", "The Beatles", 1, 100),
            rec(3, "Blue", "b1", "Joni Mitchell", 1, 300),
            rec(4, "Loose", "", "Someone", 0, 50),
            rec(5, "loose ", "", "Someone", 0, 60),
        ];
        let g = group(&recs, "");
        assert_eq!(g.iter().map(|g| g.album.as_str()).collect::<Vec<_>>(), vec!["Blue", "Abbey Road", "Loose"]);
        assert_eq!(g[1].songs.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1], "track order");
        assert_eq!(g[2].songs.len(), 2, "no album id: grouped by name");
        assert_eq!(g[1].bytes(), 2000);
        let hits = group(&recs, "beatles ROAD");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].songs.len(), 2);
        assert!(group(&recs, "nothing like this").is_empty());
    }

    #[test]
    fn an_album_shows_the_artist_most_songs_have() {
        let one = vec![rec(1, "A", "a", "X", 1, 0), rec(2, "A", "a", "x", 2, 0), rec(3, "A", "a", "Y", 3, 0)];
        assert_eq!(album_artist(&one), "X");
        let mixed = vec![rec(1, "A", "a", "X", 1, 0), rec(2, "A", "a", "Y", 2, 0), rec(3, "A", "a", "Z", 3, 0)];
        assert_eq!(album_artist(&mixed), "Various artists");
        assert_eq!(album_artist(&[rec(1, "A", "a", "", 1, 0)]), "");
    }

    #[test]
    fn the_storage_line_reads_plainly() {
        assert_eq!(storage_line(Some((1.5 * 1024.0 * 1024.0 * 1024.0, 50.0 * 1024.0 * 1024.0 * 1024.0)), 0.0, 34, Some(true)), "1.5 GB of 50.0 GB used · 34 songs · persistent");
        assert_eq!(storage_line(None, 3.0 * 1024.0 * 1024.0, 1, Some(false)), "3.0 MB · 1 song · not persistent");
        assert_eq!(storage_line(None, 0.0, 0, None), "0 songs");
    }

    #[test]
    fn the_home_screen_hint_is_for_safari_on_iphone_and_ipad() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Version/17.0 Mobile/15E148 Safari/604.1";
        assert!(ios_hint(iphone, "iPhone", 5.0, false));
        assert!(!ios_hint(iphone, "iPhone", 5.0, true), "added to the home screen");
        assert!(ios_hint("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15", "MacIntel", 5.0, false), "iPadOS");
        assert!(!ios_hint("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15", "MacIntel", 0.0, false), "a Mac");
        assert!(!ios_hint("Mozilla/5.0 (Linux; Android 14) Chrome/120", "Linux armv8l", 5.0, false));
    }

    #[test]
    fn a_saved_song_plays_with_its_library_id_and_a_converted_one_without_a_format() {
        let r = rec(7, "A", "a1", "X", 3, 0);
        let t = track_of(&r);
        assert_eq!(t["id"], 7);
        assert_eq!(t["album_id"], "a1");
        assert_eq!(t["format"], "flac");
        let c = Record { quality: "AAC 256k (converted)".into(), ext: "aac".into(), ..r.clone() };
        assert!(track_of(&c).get("format").is_none());
        assert!(again_row(&r).get("format").is_none(), "Download again lets the server's /info decide");
        assert!(track_of(&Record { album_id: String::new(), ..r }).get("album_id").is_none());
    }
}
