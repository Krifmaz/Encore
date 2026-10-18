//! Port of web/js/pages/library.js – see docs/ui-rewrite/CONTRACT.md
//! The library page (Albums / Singles / Artists / Songs / Duplicates / Wrong audio tabs with sorting, filters and
//! shuffle), the album page (play, shuffle, likes, Edit details, Tag / match metadata, artwork, delete) and the
//! per-song ⋯ menu with the Edit song details modal.

use crate::api::{self, ApiError};
use crate::app::{self, Ctx, PageFuture};
use crate::dom::{self, EventListener};
use crate::json::{js_string, json, J};
use crate::pages::{artwork, common, foryou, lyrics, playlists};
use crate::player;
use crate::ui::{self, confirm_dialog, empty_state, esc, esck, icon, modal, pop_menu, skeleton_grid, toast, MenuItem, Modal, ModalOpts};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use web_sys::Element;

const PAGE: usize = 120;

// ---- small helpers for the JS idioms used in this file ----

/// `encodeURIComponent(s)`.
fn enc(s: &str) -> String { js_sys::encode_uri_component(s).into() }
/// `window.dispatchEvent(new HashChangeEvent('hashchange'))` – re-renders the current page.
fn hashchange() {
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") { let _ = dom::window().dispatch_event(&ev); }
}
/// `view.querySelector('#lib-sub').textContent = text`.
fn set_sub(view: &Element, text: &str) { if let Some(el) = dom::qs_in(view, "#lib-sub") { dom::set_text(&el, text); } }
/// `root.querySelector(sel)` where the JS would throw a TypeError when it is missing.
fn need(root: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(root, sel).ok_or_else(|| format!("{sel} is missing")) }
/// `t.key || fallback` as a string.
fn or_str(t: &Value, key: &str, fallback: &str) -> String { if t.truthy(key) { t.s(key) } else { fallback.to_string() } }
/// `[t.a, t.b, …].filter(Boolean).join(sep)`.
fn join_truthy(t: &Value, keys: &[&str], sep: &str) -> String {
    keys.iter().filter(|k| t.truthy(k)).map(|k| t.s(k)).collect::<Vec<_>>().join(sep)
}
/// `${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}`.
fn format_text(t: &Value) -> String {
    format!("{}{}", esc(&t.s("format").to_uppercase()), if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() })
}
/// `${o.total ? o.done / o.total * 100 : 0}` for the progress bars.
fn pct(o: &Value, done: &str, total: &str) -> String {
    if o.truthy(total) { js_string(&Value::from(o.f(done) / o.f(total) * 100.0)) } else { "0".into() }
}
/// `tracks.map(t => ({ ...t, album_id: id }))`.
fn with_album_id(tracks: &[Value], id: &str) -> Vec<Value> {
    tracks.iter().map(|t| {
        let mut t = t.clone();
        if let Some(o) = t.as_object_mut() { o.insert("album_id".into(), Value::String(id.into())); }
        t
    }).collect()
}
/// `[...tracks].sort(() => Math.random() - 0.5)` – a random order.
fn shuffled(tracks: &[Value]) -> Vec<Value> {
    let mut v = tracks.to_vec();
    for i in (1..v.len()).rev() {
        let j = (js_sys::Math::random() * (i as f64 + 1.0)).floor() as usize;
        v.swap(i, j);
    }
    v
}
/// `Math.round(x * 100)`.
fn round_pct(x: f64) -> i64 { js_sys::Math::round(x * 100.0) as i64 }
/// `[...root.querySelectorAll('input')]` → `{ [name]: value.trim() }`.
fn input_values(root: &Element, sel: &str) -> Map<String, Value> {
    let mut m = Map::new();
    for i in dom::qsa_in(root, sel) {
        m.insert(i.get_attribute("name").unwrap_or_default(), Value::String(dom::value(&i).trim().to_string()));
    }
    m
}

// ---- playAlbum / sourceLine ----

pub fn play_album(id: &str, start: usize) {
    let id = id.to_string();
    dom::spawn(async move {
        let Ok(a) = api::get(&format!("/library/albums/{id}")).await else { return };
        player::play_queue(with_album_id(&a.a("tracks"), &id), start, "album");
    });
}

fn source_line(t: &Value) -> String {
    if !t.truthy("source") { return String::new(); }
    let when = if t.truthy("downloaded_at") { format!(" · {}", ui::fmt::ago(t.f("downloaded_at"))) } else { String::new() };
    let link = if t.truthy("source_url") {
        format!(r##" · <a href="{}" target="_blank" rel="noopener noreferrer" style="color:var(--accent)">link</a>"##, esck(t, "source_url"))
    } else { String::new() };
    let who = if t.truthy("requested_by") {
        format!(" · requested by {}{}", esck(t, "requested_by"), if t.truthy("requested_at") { format!(" on {}", ui::fmt::day(t.f("requested_at"))) } else { String::new() })
    } else { String::new() };
    format!(r##"<div class="small faint ellipsis" style="max-width:520px" title="{}">⬇ {}{}{link}{when}{who}</div>"##,
        esc(&join_truthy(t, &["source", "source_detail"], " – ")), esck(t, "source"),
        if t.truthy("source_detail") { format!(" · {}", esck(t, "source_detail")) } else { String::new() })
}

// ---- trackMenu ----

/// "⋯" for one song: play next / queue / playlist / download the file / more like this / delete.
pub fn track_menu(anchor: &Element, t: &Value, admin: bool, on_deleted: Option<Rc<dyn Fn()>>) {
    let t = Rc::new(t.clone());
    let done: Rc<dyn Fn()> = Rc::new(move || { if let Some(f) = &on_deleted { f(); } });
    let fix_perm = app::user().map(|u| u.g("perms").truthy("fix")).unwrap_or(false);
    let can_fix = admin || (fix_perm && t.truthy("request_id"));
    let mut items: Vec<Option<MenuItem>> = Vec::new();
    { let t = t.clone(); items.push(MenuItem::new("Play next", &icon("next"), move || { player::play_next(vec![(*t).clone()]); toast("Plays next", "ok"); })); }
    { let t = t.clone(); items.push(MenuItem::new("Add to queue", &icon("list"), move || { player::enqueue(vec![(*t).clone()]); toast("Added to queue", "ok"); })); }
    { let t = t.clone(); items.push(MenuItem::new("Add to playlist", &icon("plus"), move || playlists::add_to_playlist_modal(vec![t.i("id")]))); }
    { let t = t.clone(); items.push(MenuItem::new("More like this", &icon("spark"), move || foryou::more_like_this(&t.s("artist"), &t.s("title"), &t.s("mb_trackid")))); }
    { let t = t.clone(); items.push(MenuItem::new("Lyrics", &icon("music"), move || lyrics::lyrics_modal(&t))); }
    { let t = t.clone(); items.push(MenuItem::new("Share link", &icon("external"), move || dom::spawn(ui::share_track((*t).clone())))); }
    items.push(MenuItem::link("Download this song", &icon("download"), &format!("/api/library/tracks/{}/file", t.s("id")), true));
    if player::can_save_offline() {
        let t = t.clone();
        items.push(MenuItem::new("Save for offline", &icon("download"), move || { player::save_offline(vec![(*t).clone()]); toast("Saving to this device…", "ok"); }));
    }
    if t.truthy("album_id") { items.push(MenuItem::link("Go to album", &icon("disc"), &format!("#/library/album/{}", t.s("album_id")), false)); }
    { let (t, done) = (t.clone(), done.clone()); items.push(MenuItem::new("Report a problem", &icon("shield"), move || { let done = done.clone(); crate::pages::report::report_modal("track", t.g("id").clone(), &t.s("title"), "", Some(Rc::new(move |_| done()))) })); }
    if ui::translate_mode() != "off" { let (t, done) = (t.clone(), done.clone()); items.push(MenuItem::new("Edit translation", &icon("edit"), move || crate::pages::translate::edit_translation(t.s("title"), "title", Some(done.clone())))); }
    if admin { let t = t.clone(); items.push(MenuItem::new("Edit song details…", &icon("edit"), move || fix_song_modal(&t))); }
    if can_fix {
        if t.s("verify_status") == "manual" {
            let (t, done) = (t.clone(), done.clone());
            items.push(MenuItem::new("Undo “confirmed”", &icon("x"), move || {
                let (t, done) = (t.clone(), done.clone());
                dom::spawn(async move {
                    if api::post("/library/verify-manual", json!({ "track_ids": [t.g("id")], "correct": false })).await.is_ok() {
                        toast("Will be checked again", "ok");
                        done();
                    }
                });
            }));
        } else {
            let (t, done) = (t.clone(), done.clone());
            items.push(MenuItem::new("This is the right song", &icon("check"), move || {
                let (t, done) = (t.clone(), done.clone());
                dom::spawn(async move {
                    match api::post("/library/verify-manual", json!({ "track_ids": [t.g("id")] })).await {
                        Ok(_) => { toast("Marked as confirmed – the audio check won’t flag it again", "ok"); done(); }
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            }));
        }
        items.push(MenuItem::SEP);
        let (t, done) = (t.clone(), done.clone());
        items.push(MenuItem::danger(if admin { "Delete song from disk" } else { "Wrong song – remove it" }, &icon("trash"), move || {
            let (t, done) = (t.clone(), done.clone());
            dom::spawn(async move {
                if !confirm_dialog("Delete song", &format!("Permanently delete \"{}\" from disk?", t.s("title")), "Delete", true).await { return; }
                match api::del(&format!("/library/tracks/{}", t.s("id"))).await {
                    Ok(_) => { toast("Song deleted", "ok"); done(); }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        }));
    }
    pop_menu(anchor, items);
}

// ---- fixSongModal ----

/// Correct one song (wrong match, or a different version that got lumped in as a duplicate).
pub fn fix_song_modal(t: &Value) {
    let t = Rc::new(t.clone());
    let fields: [(&str, &str, String); 6] = [
        ("title", "Title", t.s("title")),
        ("artist", "Artist", t.s("artist")),
        ("album", "Album", or_str(&t, "album", "")),
        ("albumartist", "Album artist", if t.truthy("albumartist") { t.s("albumartist") } else { t.s("artist") }),
        ("tracknumber", "Track #", or_str(&t, "track", "")),
        ("date", "Year", or_str(&t, "year", "")),
    ];
    let body = format!(r##"<p class="small muted" style="margin-top:0">Change what this file is. Give a different version its own title (e.g. “… (Remix)”) or album so it stops counting as a duplicate. The file is moved to match and is never merged with another song.</p>
      <div class="form-grid">{}</div>
      <label class="switch mt"><input type="checkbox" id="fx-move" checked><i></i>Move the file to match (Artist/Album/NN Title)</label>
      <div class="row mt" style="gap:8px"><button class="btn sm" id="fx-id">{}Identify by its audio</button><span class="small muted" id="fx-msg"></span></div>
      <div id="fx-res" class="mt"></div>"##,
        fields.iter().map(|(k, l, v)| format!(r##"<label class="field"><span>{l}</span><input class="input" data-k="{k}" value="{}"></label>"##, esc(v))).collect::<String>(),
        icon("shield"));
    let m = modal(ModalOpts::new("Edit song details", &body)
        .footer(r##"<button class="btn" data-close>Cancel</button><button class="btn primary" id="fx-save">Save</button>"##)
        .wide(true));
    // `el('#fx-res').onclick = …` is replaced on every identify run
    let res_click: Rc<RefCell<Option<EventListener>>> = Rc::new(RefCell::new(None));
    {
        let (m, t, slot) = (m.clone(), t.clone(), res_click.clone());
        dom::on_sel(&m.el.clone(), "#fx-id", "click", move |e| {
            let target = dom::target_el(e);
            let (m, t, slot) = (m.clone(), t.clone(), slot.clone());
            dom::spawn(async move {
                if let Some(b) = &target { dom::set_disabled(b, true); }
                if let Some(msg) = m.qs("#fx-msg") { dom::set_html(&msg, r##"<span class="spinner"></span> Listening…"##); }
                match api::get(&format!("/library/tracks/{}/identify", t.s("id"))).await {
                    Ok(res) => {
                        let res = res.items();
                        if let Some(msg) = m.qs("#fx-msg") { dom::set_text(&msg, if !res.is_empty() { "Pick what it really is:" } else { "No match for this audio." }); }
                        if let Some(fr) = m.qs("#fx-res") {
                            dom::set_html(&fr, &format!(r##"<div class="list">{}</div>"##, res.iter().enumerate().map(|(i, r)| format!(r##"<button class="list-item" data-r="{i}" style="text-align:left;width:100%;border:0;background:none;color:inherit;cursor:pointer">
        <div class="grow"><div style="font-weight:600">{}</div><div class="small muted">{}{}</div></div><span class="badge ok">{}%</span></button>"##,
                                esck(r, "title"), esck(r, "artist"), if r.truthy("album") { format!(" · {}", esck(r, "album")) } else { String::new() }, r.s("score"))).collect::<String>()));
                            let res = Rc::new(res);
                            let m2 = m.clone();
                            *slot.borrow_mut() = Some(dom::listener(&fr, "click", move |ev| {
                                let Some(b) = dom::closest(ev, "[data-r]") else { return };
                                let Some(r) = dom::data(&b, "r").parse::<usize>().ok().and_then(|i| res.get(i)) else { return };
                                if let Some(i) = m2.qs("[data-k=title]") { dom::set_value(&i, &r.s("title")); }
                                if let Some(i) = m2.qs("[data-k=artist]") { dom::set_value(&i, &r.s("artist")); }
                                if r.truthy("album") { if let Some(i) = m2.qs("[data-k=album]") { dom::set_value(&i, &r.s("album")); } }
                                toast("Filled in – check and save", "ok");
                            }));
                        }
                    }
                    Err(err) => { if let Some(msg) = m.qs("#fx-msg") { dom::set_text(&msg, &err.message); } }
                }
                if let Some(b) = &target { dom::set_disabled(b, false); }
            });
        });
    }
    {
        let (m, t) = (m.clone(), t.clone());
        dom::on_sel(&m.el.clone(), "#fx-save", "click", move |e| {
            let target = dom::target_el(e);
            let (m, t) = (m.clone(), t.clone());
            dom::spawn(async move {
                if let Some(b) = &target { dom::set_disabled(b, true); }
                let mut values = Map::new();
                for i in m.qsa("[data-k]") { values.insert(dom::data(&i, "k"), Value::String(dom::value(&i))); }
                let mv = m.qs("#fx-move").map(|c| dom::checked(&c)).unwrap_or(false);
                match api::post(&format!("/library/tracks/{}/fix", t.s("id")), json!({ "values": Value::Object(values), "move": mv })).await {
                    Ok(r) => {
                        toast("Song fixed", "ok");
                        m.close();
                        let hash = dom::hash();
                        let album_id = r.s("album_id");
                        if r.truthy("album_id") && hash.contains("/library/album/") && !hash.ends_with(&album_id) { dom::set_hash(&format!("#/library/album/{album_id}")); }
                        else { hashchange(); }
                    }
                    Err(err) => { toast(&err.message, "err"); if let Some(b) = &target { dom::set_disabled(b, false); } }
                }
            });
        });
    }
}

// ---- the library page ----

/// `SORT_OPTS[tab]` (singles share the album options).
fn sort_opts(tab: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match tab {
        "albums" | "singles" => Some(&[("added", "Date added"), ("released", "Release date"), ("album", "Title A–Z"), ("artist", "Artist A–Z"), ("duration", "Length"), ("size", "Size")]),
        "artists" => Some(&[("name", "Name A–Z"), ("added", "Date added"), ("albums", "Most albums"), ("tracks", "Most songs")]),
        "tracks" => Some(&[("added", "Date added"), ("released", "Release date"), ("title", "Title A–Z"), ("artist", "Artist A–Z"), ("album", "Album A–Z"), ("duration", "Length")]),
        _ => None,
    }
}
/// `DESC_FIRST.has(sort)`.
fn desc_first(sort: &str) -> bool { matches!(sort, "added" | "released" | "duration" | "size" | "albums" | "tracks") }

/// `a.cover_v` as URL text ("" when there is none).
fn cover_v_text(a: &Value) -> String {
    match a.get("cover_v") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The JS `st` object.
struct St { tab: String, q: String, sort: String, dir: String, filter: String, artist: String, offset: usize }

/// Everything the closures of `render` shared.
struct Lib {
    view: Element,
    body: Element,
    more: Element,
    sort_el: Element,
    dir_el: Element,
    admin: bool,
    /// `ctx.query.tab`
    query_tab: String,
    st: RefCell<St>,
    /// `encore.libSort`
    saved: RefCell<Value>,
    /// `body._tracks`
    tracks: RefCell<Vec<Value>>,
    /// `body.onclick` (set by the duplicates tab; replaced, not stacked)
    body_onclick: RefCell<Option<EventListener>>,
    /// the side scrubber (web/js/scrubber.js) and `indexSeq`
    scrub: RefCell<Option<Rc<crate::scrubber::Scrubber>>>,
    index_seq: Cell<u32>,
    /// `nextPage`: the albums page after the last one shown, fetched ahead (url, the answer once it came)
    next_page: Rc<RefCell<Option<(String, Option<Value>)>>>,
}

impl Lib {
    fn paint_sort(&self) {
        let tab = self.st.borrow().tab.clone();
        let opts = sort_opts(&tab);
        if let Some(p) = self.sort_el.parent_element() { dom::toggle_class(&p, "hidden", opts.is_none()); }
        let Some(opts) = opts else { return };
        let saved = self.saved.borrow();
        let sv = saved.g(&tab);
        let mut st = self.st.borrow_mut();
        let cur = if tab == self.query_tab && !st.sort.is_empty() { st.sort.clone() }
            else if let Some(s) = sv.st("sort") { s.to_string() }
            else { opts[0].0.to_string() };
        st.sort = if opts.iter().any(|o| o.0 == cur) { cur } else { opts[0].0.to_string() };
        st.dir = if sv.g("sort").as_str() == Some(st.sort.as_str()) && sv.truthy("dir") { sv.s("dir") }
            else if desc_first(&st.sort) { "desc".into() } else { "asc".into() };
        dom::set_html(&self.sort_el, &opts.iter().map(|(k, l)| format!(r##"<option value="{k}">Sort: {l}</option>"##)).collect::<String>());
        dom::set_value(&self.sort_el, &st.sort);
        dom::set_text(&self.dir_el, if st.dir == "asc" { "↑" } else { "↓" });
    }

    fn save_sort(&self) {
        let st = self.st.borrow();
        let mut saved = self.saved.borrow_mut();
        saved[st.tab.as_str()] = json!({ "sort": st.sort, "dir": st.dir });
        dom::ls_set("encore.libSort", &saved.to_string());
    }

    /// `dirEl.textContent = st.dir === 'asc' ? '↑' : '↓'`.
    fn paint_dir(&self) { dom::set_text(&self.dir_el, if self.st.borrow().dir == "asc" { "↑" } else { "↓" }); }

    /// `load()` as the handlers call it (not awaited; a failure only reaches the console).
    fn reload(self: &Rc<Self>) {
        let me = self.clone();
        dom::spawn(async move { let _ = me.load(false, 0).await; });
    }

    /// `rowsOf()[i]`: the list's row `i`.
    fn row(&self, i: usize) -> Option<Element> {
        let sel = if self.st.borrow().tab == "tracks" { "tbody tr" } else { ".grid > *" };
        wasm_bindgen::JsCast::dyn_into::<Element>(self.body.query_selector_all(sel).ok()?.item(i as u32)?).ok()
    }
    fn row_count(&self) -> usize {
        let sel = if self.st.borrow().tab == "tracks" { "tbody tr" } else { ".grid > *" };
        self.body.query_selector_all(sel).map(|l| l.length() as usize).unwrap_or(0)
    }
    /// `jump(offset)`: loads up to that row with the list's paging, then scrolls to it.
    async fn jump(self: Rc<Self>, offset: usize) {
        while self.row_count() <= offset && !dom::has_class(&self.more, "hidden") {
            let n = self.row_count();
            if self.clone().load(true, offset + 1 - n).await.is_err() || self.row_count() == n { break; }
        }
        if let Some(r) = self.row(offset) {
            let w = dom::window();
            w.scroll_to_with_x_and_y(w.scroll_x().unwrap_or(0.0), r.get_bounding_client_rect().top() + w.scroll_y().unwrap_or(0.0) - 16.0);
        }
    }
    /// `loadIndex()`: the scrubber's sections for the list as it is sorted (none while filtering).
    fn load_index(self: &Rc<Self>) {
        let seq = self.index_seq.get() + 1;
        self.index_seq.set(seq);
        let Some(scrub) = self.scrub.borrow().clone() else { return };
        scrub.set(&Value::Null);
        let (tab, q, sort, dir, filter, artist) = {
            let s = self.st.borrow();
            (s.tab.clone(), s.q.clone(), s.sort.clone(), s.dir.clone(), s.filter.clone(), s.artist.clone())
        };
        if !matches!(tab.as_str(), "albums" | "singles" | "artists" | "tracks") || !q.is_empty() { return; }
        let kind = if tab == "albums" { "albums&album_kind=albums".to_string() } else { tab };
        let me = self.clone();
        dom::spawn(async move {
            if let Ok(sections) = api::get(&format!("/library/index?kind={kind}&sort={sort}&dir={dir}&filter={filter}&artist={}", enc(&artist))).await {
                if me.index_seq.get() == seq { scrub.set(&sections); }
            }
        });
    }

    async fn load(self: Rc<Self>, append: bool, want: usize) -> Result<(), ApiError> {
        if !append { self.st.borrow_mut().offset = 0; dom::set_html(&self.body, &skeleton_grid(12)); }
        let tab = self.st.borrow().tab.clone();
        if let Some(filt) = dom::qs_in(&self.view, "#lib-filter") { dom::toggle_class(&filt, "hidden", !matches!(tab.as_str(), "albums" | "singles")); }
        if let Some(p) = dom::qs_in(&self.view, "#lib-q").and_then(|q| q.parent_element()) { dom::toggle_class(&p, "hidden", matches!(tab.as_str(), "duplicates" | "issues")); }
        if !append { self.load_index(); }
        if tab == "duplicates" { return self.clone().render_duplicates().await; }
        if tab == "issues" { return render_issues(&self.body, &self.view, &self.more).await; }
        let (q, sort, dir, filter, artist, offset) = {
            let s = self.st.borrow();
            (s.q.clone(), s.sort.clone(), s.dir.clone(), s.filter.clone(), s.artist.clone(), s.offset)
        };
        if tab == "albums" || tab == "singles" {
            let page_url = |offset: usize, limit: usize| format!("/library/albums?q={}&sort={sort}&dir={dir}&kind={tab}&filter={filter}&artist={}&limit={limit}&offset={offset}", enc(&q), enc(&artist));
            let url = page_url(offset, if want > 0 { want.clamp(PAGE, 500) } else { PAGE });
            // the next page was fetched ahead (with its covers) when this one showed
            let ahead = self.next_page.borrow_mut().take().filter(|(u, _)| append && *u == url).and_then(|(_, r)| r);
            let res = match ahead { Some(r) => r, None => api::get(&url).await? };
            let items = res.a("items");
            let html: String = items.iter().map(common::library_tile).collect();
            if append {
                if let Some(g) = dom::qs_in(&self.body, ".grid") { let _ = g.insert_adjacent_html("beforeend", &html); }
            } else {
                dom::set_html(&self.body, &if !items.is_empty() { format!(r##"<div class="grid">{html}</div>"##) }
                    else { empty_state(&icon("library"), "No albums found", if filter == "untagged" { "Everything is tagged 🎉" } else { "" }) });
            }
            let off = { let mut s = self.st.borrow_mut(); s.offset += items.len(); s.offset };
            dom::toggle_class(&self.more, "hidden", off as f64 >= res.f("total"));
            if (off as f64) < res.f("total") {
                let u = page_url(off, PAGE);
                *self.next_page.borrow_mut() = Some((u.clone(), None));
                let slot = self.next_page.clone();
                dom::spawn(async move {
                    let Ok(r) = api::get(&u).await else { return };
                    ui::preload_covers(&r.a("items").iter().map(|a| ui::cover_url(&a.s("id"), &cover_v_text(a), 400)).collect::<Vec<_>>());
                    if let Some(p) = slot.borrow_mut().as_mut().filter(|p| p.0 == u) { p.1 = Some(r); }
                });
            }
            set_sub(&self.view, &format!("{} {tab}", ui::fmt::num(res.f("total"))));
        } else if tab == "artists" {
            let rows = api::get(&format!("/library/artists?q={}&sort={sort}&dir={dir}", enc(&q))).await?.items();
            dom::add_class(&self.more, "hidden");
            set_sub(&self.view, &format!("{} artists", ui::fmt::num(rows.len() as f64)));
            dom::set_html(&self.body, &if !rows.is_empty() { format!(r##"<div class="grid">{}</div>"##, rows.iter().map(|a| self.artist_tile(a)).collect::<String>()) }
                else { empty_state(&icon("users"), "No artists found", "") });
        } else {
            let lim = if want > 0 { want.clamp(300, 1000) } else { 300 };
            let rows = api::get(&format!("/library/tracks?q={}&sort={sort}&dir={dir}&limit={lim}&offset={}", enc(&q), if append { offset } else { 0 })).await?.items();
            let base = if append { self.tracks.borrow().len() } else { 0 };
            let html: String = rows.iter().enumerate().map(|(i, t)| format!(r##"<tr data-i="{idx}">
        <td style="width:52px"><button class="thumb play-thumb" data-play-track="{id}" aria-label="Play">{art}<i>{play}</i></button></td>
        <td><div style="font-weight:600" class="ellipsis">{title}</div>{title_sub}<div class="small muted ellipsis">{artist}<span class="show-sm"> · {album_ln}</span></div></td>
        <td class="hide-sm"><a href="#/library/album/{album_id}">{album}</a></td>
        <td class="hide-sm"><span class="badge">{fmt}</span> {vb}</td>
        <td class="muted hide-sm">{dur}</td>
        <td style="text-align:right"><button class="btn sm ghost icon" data-tmenu="{idx}" aria-label="More">{more}</button></td></tr>"##,
                idx = base + i, id = t.s("id"),
                art = ui::art_sized(&format!("/api/library/albums/{}/cover{}", t.s("album_id"), ui::cover_q(t)), &if t.truthy("album") { t.s("album") } else { t.s("title") }, "", 200),
                play = icon("play"), title = esc(&ui::tr(t, "title")), title_sub = ui::tr_sub(t, "title"), artist = ui::artist_links(t), album_ln = ui::album_link(t), album = esc(&ui::tr_inline(t, "album")), album_id = esck(t, "album_id"),
                fmt = format_text(t), vb = verify_badge(t), dur = ui::fmt::dur(t.f("duration")), more = icon("more"))).collect();
            if !append {
                dom::set_html(&self.body, &if !rows.is_empty() {
                    format!(r##"<div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th></th><th>Title</th><th class="hide-sm">Album</th><th class="hide-sm">Format</th><th class="hide-sm">Time</th><th></th></tr></thead><tbody>{html}</tbody></table></div></div>"##)
                } else { empty_state(&icon("music"), "No songs found", "") });
                *self.tracks.borrow_mut() = rows.clone();
            } else {
                if let Some(tb) = dom::qs_in(&self.body, "tbody") { let _ = tb.insert_adjacent_html("beforeend", &html); }
                self.tracks.borrow_mut().extend(rows.iter().cloned());
            }
            { let mut s = self.st.borrow_mut(); s.offset = (if append { s.offset } else { 0 }) + rows.len(); }
            dom::toggle_class(&self.more, "hidden", rows.len() < lim);
            set_sub(&self.view, "");
        }
        if let Some(scrub) = self.scrub.borrow().as_ref() { scrub.refresh(); }
        Ok(())
    }

    /// One artist of the Artists tab.
    fn artist_tile(&self, a: &Value) -> String {
        format!(r##"
        <div class="tile round" data-href="{href}">
          <div class="art">{art}
            {rm}</div>
          <div class="t ellipsis" style="text-align:center">{name_shown}</div>
          <div class="s" style="text-align:center">{albums} album{s} · {tracks} tracks</div>
        </div>"##,
            href = if a.truthy("mbid") { format!("/artist/{}", esck(a, "mbid")) } else { format!("/artist-name/{}", enc(&a.s("name"))) },
            art = ui::art(&format!("/api/library/albums/{}/cover{}", a.s("cover_album"), ui::cover_q(a)), &a.s("name"), ""),
            rm = if self.admin { format!(r##"<div class="overlay"><button class="fab" data-rm-artist="{}" title="Remove this artist from the library">{}</button></div>"##, esck(a, "name"), icon("trash")) } else { String::new() },
            name_shown = esc(&ui::tr_inline(a, "name")), albums = a.s("albums"), s = if a.num("albums") == Some(1.0) { "" } else { "s" }, tracks = a.s("tracks"))
    }

    async fn render_duplicates(self: Rc<Self>) -> Result<(), ApiError> {
        dom::add_class(&self.more, "hidden");
        dom::set_html(&self.body, r##"<div class="empty"><span class="spinner"></span><p class="muted">Comparing fingerprints, ISRCs, MusicBrainz ids and tags…</p></div>"##);
        let groups = api::get("/library/duplicates").await?.items();
        set_sub(&self.view, &format!("{} duplicate group(s)", groups.len()));
        if groups.is_empty() {
            dom::set_html(&self.body, &empty_state(&icon("check"), "No duplicates found", "Remixes, live versions, covers and edits are kept apart – only true copies of the same recording are listed here. Run “Verify audio” to also catch copies with different tags."));
            return Ok(());
        }
        let cards: String = groups.iter().map(|g| {
            let tracks = g.a("tracks");
            let first = tracks.first().cloned().unwrap_or(Value::Null);
            format!(r##"<div class="card flat mt" data-key="{key}">
      <div class="row between" style="padding:12px 16px;border-bottom:1px solid var(--border)">
        <div><b>{artist} – {title}</b> <span class="badge accent">{reason}</span></div>
        <div class="row"><button class="btn sm ghost" data-ignore>Not duplicates</button><button class="btn sm danger" data-resolve>Delete unchecked</button></div>
      </div>
      <div class="list">{list}</div></div>"##,
                key = esck(g, "key"), artist = esck(&first, "artist"), title = esck(&first, "title"), reason = esck(g, "reason"),
                list = tracks.iter().map(|t| format!(r##"<label class="list-item" style="cursor:pointer">
        <input type="radio" name="keep-{key}" value="{id}" {checked}>
        <div class="grow"><div class="ellipsis">{title} <span class="muted">· {album}</span></div>
          <div class="small faint ellipsis">{relpath}</div></div>
        <span class="badge">{fmt}</span>
        {vb}<span class="small muted">{dur} · {size}</span>
      </label>"##,
                    key = esck(g, "key"), id = t.s("id"), checked = if t.g("id") == g.g("keep") { "checked" } else { "" },
                    title = esck(t, "title"), album = esck(t, "album"), relpath = esck(t, "relpath"), fmt = format_text(t), vb = verify_badge(t),
                    dur = ui::fmt::dur(t.f("duration")), size = ui::fmt::size(t.f("size")))).collect::<String>())
        }).collect();
        dom::set_html(&self.body, &format!(r##"
    <div class="card row wrap between" style="margin-bottom:16px">
      <div class="muted small">The best copy (verified → lossless → bitrate → tagged) is preselected to keep. Playlists pointing at removed copies are moved to the kept one.</div>
      <button class="btn danger" id="dup-all">{}Resolve all {n}</button>
    </div>
    {cards}"##, icon("trash"), n = groups.len()));
        {
            let (me, n) = (self.clone(), groups.len());
            dom::on_sel(&self.body, "#dup-all", "click", move |_| {
                let me = me.clone();
                dom::spawn(async move {
                    if !confirm_dialog("Resolve all duplicates", &format!("Delete the lower-quality copies in {n} groups? Files are removed from disk."), "Delete copies", true).await { return; }
                    let Ok(r) = api::post_empty("/library/duplicates/resolve-all").await else { return };
                    toast(&format!("Removed {} duplicate file(s)", r.s("deleted")), "ok");
                    me.reload();
                });
            });
        }
        *self.body_onclick.borrow_mut() = Some(dom::listener(&self.body, "click", move |e| {
            let Some(card) = dom::closest(e, "[data-key]") else { return };
            let key = dom::data(&card, "key");
            if dom::closest(e, "[data-ignore]").is_some() {
                dom::spawn(async move {
                    if api::post("/library/duplicates/ignore", json!({ "key": key })).await.is_ok() { card.remove(); }
                });
            } else if dom::closest(e, "[data-resolve]").is_some() {
                let Some(ck) = dom::qs_in(&card, "input:checked") else { return };
                let keep: i64 = dom::value(&ck).parse().unwrap_or(0);
                let del: Vec<i64> = dom::qsa_in(&card, "input").iter().map(|i| dom::value(i).parse::<i64>().unwrap_or(0)).filter(|v| *v != keep).collect();
                dom::spawn(async move {
                    let n = del.len();
                    if api::post("/library/duplicates/resolve", json!({ "key": key, "keep": keep, "delete": del })).await.is_ok() {
                        toast(&format!("Removed {n} copy/copies"), "ok");
                        card.remove();
                    }
                });
            }
        }));
        Ok(())
    }
}

/// The admin status poll: fills `#bulk`, the Verify / Auto-tag buttons; resolves to "something is running".
async fn poll(view: &Element, bulk: &Element) -> Result<bool, ApiError> {
    let s = api::get("/library/stats").await?;
    let scan = s.g("scan");
    let scanning = scan.truthy("scanning");
    let b = s.g("bulk");
    let org = s.o("organize");
    let verify = s.g("verify");
    let art_running = s.truthy("artwork") && s.g("artwork").truthy("running");
    dom::toggle_class(bulk, "hidden", !scanning && !b.truthy("running") && !verify.truthy("running") && !art_running && !org.truthy("running"));
    let vc = s.o("verify_counts");
    if let Some(vb) = dom::qs_in(view, "#verify-btn") {
        dom::set_html(&vb, &format!("{}Verify audio{}{}", icon("shield"),
            if vc.truthy("unchecked") { format!(r##" <span class="badge">{} unchecked</span>"##, ui::fmt::num(vc.f("unchecked"))) } else { String::new() },
            if vc.truthy("mismatch") { format!(r##" <span class="badge err">{} wrong</span>"##, vc.s("mismatch")) } else { String::new() }));
        dom::set_data(&vb, "acoustid", if s.truthy("acoustid") { "1" } else { "" });
    }
    if scanning {
        dom::set_html(bulk, &format!(r##"<div class="row"><span class="spinner"></span><b>Scanning library</b><span class="muted">{} / {} files</span></div><div class="progress mt"><i style="width:{}%"></i></div>"##,
            scan.s("progress"), scan.s("total"), pct(scan, "progress", "total")));
    } else if art_running {
        let aw = s.g("artwork");
        dom::set_html(bulk, &format!(r##"<div class="row"><span class="spinner"></span><b>Finding artwork</b><span class="muted">{} / {} · {} found</span></div><div class="progress mt"><i style="width:{}%"></i></div>"##,
            aw.s("done"), aw.s("total"), aw.s("found"), pct(aw, "done", "total")));
    } else if verify.truthy("running") {
        dom::set_html(bulk, &format!(r##"<div class="row"><span class="spinner"></span><b>Verifying audio fingerprints</b><span class="muted ellipsis">{} / {} · {} ok · {} wrong · {} unknown · {}</span></div><div class="progress mt"><i style="width:{}%"></i></div>"##,
            verify.s("done"), verify.s("total"), verify.s("verified"), verify.s("mismatch"), verify.s("unknown"), esck(verify, "current"), pct(verify, "done", "total")));
    } else if org.truthy("running") {
        dom::set_html(bulk, &format!(r##"<div class="row"><span class="spinner"></span><b>Organizing files</b><span class="muted ellipsis">{} / {} · {} moved · {} re-tagged · {}</span></div><div class="progress mt"><i style="width:{}%"></i></div>"##,
            org.s("done"), org.s("total"), org.s("moved"), org.s("fixed"), esck(&org, "current"), pct(&org, "done", "total")));
    } else if b.truthy("running") {
        dom::set_html(bulk, &format!(r##"<div class="row"><span class="spinner"></span><b>Auto-tagging</b><span class="muted ellipsis">{} / {} · {} tagged · {} skipped · {}</span></div><div class="progress mt"><i style="width:{}%"></i></div>"##,
            b.s("done"), b.s("total"), b.s("tagged"), b.s("skipped"), esck(b, "current"), pct(b, "done", "total")));
    }
    if let Some(ab) = dom::qs_in(view, "#autotag-btn") {
        dom::set_html(&ab, &format!("{}Auto-tag untagged{}", icon("tag"),
            if s.truthy("untagged_albums") { format!(r##" <span class="badge warn">{}</span>"##, s.s("untagged_albums")) } else { String::new() }));
    }
    Ok(scanning || b.truthy("running") || verify.truthy("running") || art_running || org.truthy("running"))
}

/// `poll()` fired and forgotten after one of the admin actions.
fn poll_later(view: &Element, bulk: &Element) {
    let (view, bulk) = (view.clone(), bulk.clone());
    dom::spawn(async move { let _ = poll(&view, &bulk).await; });
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let tab = { let t = ctx.q("tab"); if t.is_empty() { "albums".to_string() } else { t } };
        let st = St { tab, q: ctx.q("q"), sort: ctx.q("sort"), dir: ctx.q("dir"), filter: ctx.q("filter"), artist: ctx.q("artist"), offset: 0 };
        let mut tabs: Vec<(&str, &str)> = vec![("albums", "Albums"), ("singles", "Singles"), ("artists", "Artists"), ("tracks", "Songs")];
        if admin { tabs.extend([("duplicates", "Duplicates"), ("issues", "Wrong audio")]); }
        let tabs_html: String = tabs.iter().map(|(k, l)| format!(r##"<button data-t="{k}" class="{}">{l}</button>"##, if *k == st.tab { "active" } else { "" })).collect();
        let admin_html = if admin {
            format!(r##"<div class="row wrap">
        <button class="btn sm" id="scan-btn">{}Scan</button>
        <button class="btn sm" id="autotag-btn">{}Auto-tag untagged</button>
        <button class="btn sm" id="organize-btn" title="Move every file into Artist/Album/NN Title folders (your naming template), filling in missing tags first">{}Organize files</button>
        <button class="btn sm" id="verify-btn">{}Verify audio</button>
        <button class="btn sm" id="art-all">{}Find missing artwork</button>
      </div>"##, icon("refresh"), icon("tag"), icon("disc"), icon("shield"), icon("disc"))
        } else { String::new() };
        let artist_html = if !st.artist.is_empty() {
            format!(r##"<span class="badge accent">{} <a href="#/library" style="margin-left:4px">✕</a></span><button class="btn sm" id="artist-share" title="Share link">{}Share</button>"##, esc(&st.artist), icon("external"))
        } else { String::new() };
        dom::set_html(&view, &format!(r##"
    <div class="row wrap between" style="margin-bottom:18px">
      <div class="row" style="gap:12px;align-items:center"><div><h1>Library</h1><div class="muted small" id="lib-sub"></div></div>
        <button class="btn primary sm" id="shuffle-lib" title="Play everything in random order – no song twice this session">{shuffle}Shuffle</button></div>
      {admin_html}
    </div>
    <div id="bulk" class="hidden card" style="margin-bottom:16px"></div>
    <div class="row wrap" style="margin-bottom:18px">
      <div class="tabs" id="lib-tabs">
        {tabs_html}
      </div>
      <div class="search grow" style="max-width:360px">{search}<input class="input" id="lib-q" placeholder="Filter library…" value="{q}"></div>
      <div class="row" style="gap:6px"><select class="input" id="lib-sort" style="width:auto"></select>
        <button class="btn sm dir-btn" id="lib-dir" title="Ascending / descending"></button></div>
      <select class="input" id="lib-filter" style="width:auto">
        <option value="">All albums</option><option value="untagged">Needs tagging</option>
        <option value="tagged">MusicBrainz tagged</option><option value="lossless">Lossless only</option>
      </select>
      {artist_html}
    </div>
    <div id="lib-body"></div>
    <div class="row mt" style="justify-content:center"><button class="btn hidden" id="more">Load more</button></div>"##,
            shuffle = icon("shuffle"), search = icon("search"), q = esc(&st.q)));
        dom::on_sel(&view, "#shuffle-lib", "click", |_| dom::spawn(async { if player::shuffle_library().await == 0 { toast("Your library is empty", "err"); } }));
        { let name = st.artist.clone(); dom::on_sel(&view, "#artist-share", "click", move |_| dom::spawn(ui::share_artist(name.clone()))); }
        let body = need(&view, "#lib-body")?;
        let more = need(&view, "#more")?;
        let saved = dom::ls_get("encore.libSort").and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
        let sort_el = need(&view, "#lib-sort")?;
        let dir_el = need(&view, "#lib-dir")?;
        let lib = Rc::new(Lib {
            view: view.clone(), body: body.clone(), more: more.clone(), sort_el: sort_el.clone(), dir_el: dir_el.clone(),
            admin, query_tab: ctx.q("tab"), st: RefCell::new(st), saved: RefCell::new(saved), tracks: RefCell::new(Vec::new()), body_onclick: RefCell::new(None),
            scrub: RefCell::new(None), index_seq: Cell::new(0), next_page: Rc::new(RefCell::new(None)),
        });
        lib.paint_sort();
        if let Some(f) = dom::qs_in(&view, "#lib-filter") { let filter = lib.st.borrow().filter.clone(); dom::set_value(&f, &filter); }
        common::bind_tiles(&body);
        {
            let (w1, w2, w3) = (Rc::downgrade(&lib), Rc::downgrade(&lib), Rc::downgrade(&lib));
            let jump: crate::scrubber::JumpFn = Rc::new(move |offset| {
                let w = w3.clone();
                Box::pin(async move { if let Some(lib) = w.upgrade() { lib.jump(offset).await; } })
            });
            let scrub = crate::scrubber::Scrubber::mount(
                &view,
                move |i| w1.upgrade().and_then(|l| l.row(i)),
                move || w2.upgrade().is_some_and(|l| {
                    let doc_h = dom::document().document_element().map(|d| d.scroll_height() as f64).unwrap_or(0.0);
                    !dom::has_class(&l.more, "hidden") || doc_h > 2.0 * dom::window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
                }),
                jump,
            );
            *lib.scrub.borrow_mut() = Some(scrub);
        }

        {
            let lib = lib.clone();
            dom::on(&body, "click", move |e| {
                if let Some(rm) = dom::closest(e, "[data-rm-artist]") {
                    dom::stop(e);
                    let name = dom::data(&rm, "rmArtist");
                    let lib = lib.clone();
                    dom::spawn(async move {
                        if !confirm_dialog("Remove artist", &format!("Delete every album by “{name}” from disk and cancel their pending downloads? Use this for artists that were added by a wrong download."), "Remove", true).await { return; }
                        match api::del(&format!("/library/artists?name={}", enc(&name))).await {
                            Ok(r) => { toast(&format!("Removed {} song(s)", r.s("removed")), "ok"); lib.reload(); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                    return;
                }
                if let Some(mb) = dom::closest(e, "[data-tmenu]") {
                    let t = dom::data(&mb, "tmenu").parse::<usize>().ok().and_then(|i| lib.tracks.borrow().get(i).cloned());
                    if let Some(t) = t {
                        let l2 = lib.clone();
                        track_menu(&mb, &t, lib.admin, Some(Rc::new(move || l2.reload())));
                    }
                    return;
                }
                let Some(b) = dom::closest(e, "[data-play-track]") else { return };
                let id = dom::data(&b, "playTrack");
                let t = lib.tracks.borrow().iter().find(|t| t.s("id") == id).cloned();
                if let Some(t) = t { player::play_queue(vec![t], 0, "single"); }   // a single song: library shuffle continues after it
            });
        }
        {
            let lib = lib.clone();
            dom::on_sel(&view, "#lib-tabs", "click", move |e| {
                let Some(t) = dom::closest(e, "[data-t]") else { return };
                lib.st.borrow_mut().tab = dom::data(&t, "t");
                for b in dom::qsa_in(&lib.view, "#lib-tabs button") { dom::toggle_class(&b, "active", b == t); }
                lib.paint_sort();
                lib.reload();
            });
        }
        if let Some(q_el) = dom::qs_in(&view, "#lib-q") {
            let (lib2, q2) = (lib.clone(), q_el.clone());
            let deb = ui::Debounce::new(300, move || { lib2.st.borrow_mut().q = dom::value(&q2).trim().to_string(); lib2.reload(); });
            dom::on(&q_el, "input", move |_| deb.run());
        }
        {
            let lib = lib.clone();
            dom::on(&sort_el, "change", move |_| {
                { let mut st = lib.st.borrow_mut(); st.sort = dom::value(&lib.sort_el); st.dir = if desc_first(&st.sort) { "desc".into() } else { "asc".into() }; }
                lib.paint_dir();
                lib.save_sort();
                lib.reload();
            });
        }
        {
            let lib = lib.clone();
            dom::on(&dir_el, "click", move |_| {
                { let mut st = lib.st.borrow_mut(); st.dir = if st.dir == "asc" { "desc".into() } else { "asc".into() }; }
                lib.paint_dir();
                lib.save_sort();
                lib.reload();
            });
        }
        {
            let lib = lib.clone();
            dom::on_sel(&view, "#lib-filter", "change", move |e| {
                if let Some(el) = dom::target_el(e) { lib.st.borrow_mut().filter = dom::value(&el); }
                lib.reload();
            });
        }
        {
            let lib = lib.clone();
            dom::on(&more, "click", move |_| { let lib = lib.clone(); dom::spawn(async move { let _ = lib.load(true, 0).await; }); });
        }

        if admin {
            let bulk = need(&view, "#bulk")?;
            let was_busy = Rc::new(Cell::new(poll(&view, &bulk).await.map_err(|e| e.message)?));
            {
                let (lib, view, bulk, wb) = (lib.clone(), view.clone(), bulk.clone(), was_busy.clone());
                dom::interval(&view.clone(), 2500, move || {
                    let (lib, view, bulk, wb) = (lib.clone(), view.clone(), bulk.clone(), wb.clone());
                    dom::spawn(async move {
                        let busy = poll(&view, &bulk).await.unwrap_or(false);
                        if wb.get() && !busy { lib.reload(); }
                        wb.set(busy);
                    });
                });
            }
            dom::on_sel(&view, "#organize-btn", "click", |_| dom::spawn(async {
                if !confirm_dialog("Organize files", "Every song is moved into folders from its tags using your naming template (Settings → Library, tagging & IDs), e.g. Artist/Album (Year)/01 - Title. Missing tags are filled in first from the file name, AcoustID and Deezer. Playlists and likes keep working.", "Organize", false).await { return; }
                match api::post_empty("/library/organize").await {
                    Ok(_) => { toast("Organizing…", "ok"); dom::timeout(800, hashchange); }
                    Err(e) => toast(&e.message, "err"),
                }
            }));
            {
                let (view, bulk, wb) = (view.clone(), bulk.clone(), was_busy.clone());
                dom::on_sel(&view.clone(), "#scan-btn", "click", move |_| {
                    let (view, bulk, wb) = (view.clone(), bulk.clone(), wb.clone());
                    dom::spawn(async move {
                        match api::post_empty("/library/scan").await {
                            Ok(_) => { toast("Library scan started", "ok"); wb.set(true); poll_later(&view, &bulk); }
                            Err(e) => toast(&e.message, "err"),
                        }
                    });
                });
            }
            {
                let (view, bulk, wb) = (view.clone(), bulk.clone(), was_busy.clone());
                dom::on_sel(&view.clone(), "#art-all", "click", move |_| {
                    let (view, bulk, wb) = (view.clone(), bulk.clone(), wb.clone());
                    dom::spawn(async move {
                        match api::post_empty("/library/artwork").await {
                            Ok(_) => { toast("Looking for missing covers (Cover Art Archive, Deezer, iTunes)…", "ok"); wb.set(true); poll_later(&view, &bulk); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                });
            }
            {
                let (view, bulk, wb) = (view.clone(), bulk.clone(), was_busy.clone());
                let btn = need(&view, "#verify-btn")?;
                let b2 = btn.clone();
                dom::on(&btn, "click", move |_| {
                    if dom::data(&b2, "acoustid").is_empty() {
                        toast("Add a free AcoustID API key in Settings → Verification first", "err");
                        return;
                    }
                    let (view, bulk, wb) = (view.clone(), bulk.clone(), wb.clone());
                    dom::spawn(async move {
                        if !confirm_dialog("Verify audio", "Every unchecked file is fingerprinted and compared against AcoustID/MusicBrainz, so wrong songs, remixes posing as originals, covers and mislabeled tracks get flagged. Takes ~0.4s per track.", "Start", false).await { return; }
                        match api::post_empty("/library/verify").await {
                            Ok(_) => { toast("Verification started", "ok"); wb.set(true); poll_later(&view, &bulk); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                });
            }
            {
                let (view, bulk, wb) = (view.clone(), bulk.clone(), was_busy.clone());
                dom::on_sel(&view.clone(), "#autotag-btn", "click", move |_| {
                    let (view, bulk, wb) = (view.clone(), bulk.clone(), wb.clone());
                    dom::spawn(async move {
                        if !confirm_dialog("Auto-tag library", "Every album without MusicBrainz tags will be matched automatically. Confident matches get retagged and renamed using your naming template; the rest are left for manual matching.", "Start", false).await { return; }
                        match api::post_empty("/library/autotag").await {
                            Ok(_) => { toast("Auto-tagging started", "ok"); wb.set(true); poll_later(&view, &bulk); }
                            Err(e) => toast(&e.message, "err"),
                        }
                    });
                });
            }
        }
        lib.clone().load(false, 0).await.map_err(|e| e.message)?;
        Ok(())
    })
}

fn verify_badge(t: &Value) -> String {
    match t.g("verify_status").as_str() {
        Some("manual") => format!(r##"<span class="badge ok" title="{}">✓ confirmed</span>"##, esc(&or_str(t, "verify_detail", "Confirmed by hand"))),
        Some("verified") => format!(r##"<span class="badge ok" title="{}">✓ verified</span>"##, esck(t, "verify_detail")),
        Some("mismatch") => format!(r##"<span class="badge err" title="{}">wrong audio</span>"##, esck(t, "verify_detail")),
        _ => String::new(),
    }
}

async fn render_issues(body: &Element, view: &Element, more: &Element) -> Result<(), ApiError> {
    dom::add_class(more, "hidden");
    let rows = api::get("/library/mismatches").await?.items();
    set_sub(view, &format!("{} file(s) whose audio doesn't match their tags", rows.len()));
    dom::set_html(body, &if !rows.is_empty() {
        format!(r##"<div class="card flat"><div class="list">{}</div></div>"##, rows.iter().map(|t| format!(r##"<div class="list-item">
      <div class="grow"><div><b>{artist} – {title}</b> <span class="muted">· <a href="#/library/album/{aid}">{album}</a></span></div>
      <div class="small" style="color:var(--err)">{detail}</div></div>
      <a class="btn sm" href="#/library/album/{aid}">{tag}Fix</a>
    </div>"##, artist = esck(t, "artist"), title = esck(t, "title"), aid = esck(t, "album_id"), album = esck(t, "album"), detail = esck(t, "verify_detail"), tag = icon("tag"))).collect::<String>())
    } else { empty_state(&icon("shield"), "Nothing flagged", "Run “Verify audio” to fingerprint your library.") });
    Ok(())
}

// ---- the album page ----

pub fn render_album(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let id = ctx.param(0);
        let a = api::get(&format!("/library/albums/{id}")).await.map_err(|e| e.message)?;
        let tracks = a.a("tracks");
        let multi = tracks.iter().map(|t| t.g("disc").to_string()).collect::<HashSet<_>>().len() > 1;
        let mut formats: Vec<String> = Vec::new();
        for t in &tracks { let f = t.s("format").to_uppercase(); if !formats.contains(&f) { formats.push(f); } }
        let formats = formats.join(" / ");
        let untagged = tracks.iter().filter(|t| !t.truthy("mb_albumid")).count();
        let requesters = { let mut v: Vec<String> = vec![]; for t in &tracks { let w = t.s("requested_by"); if !w.is_empty() && !v.contains(&w) { v.push(w); } } v.join(", ") };
        // `Math.min(...dates)` – infinity when there are none
        let earliest = |key: &str| tracks.iter().filter(|t| t.truthy(key)).map(|t| t.f(key)).fold(f64::INFINITY, f64::min);
        let (requested_at, added_at) = (earliest("requested_at"), earliest("added_at"));
        let history = [
            if requesters.is_empty() { String::new() } else { format!("Requested by {}{}", esc(&requesters), if requested_at.is_finite() { format!(" on {}", ui::fmt::day(requested_at)) } else { String::new() }) },
            if added_at.is_finite() { format!("{} {}", if requesters.is_empty() { "Added" } else { "added" }, ui::fmt::day(added_at)) } else { String::new() },
        ].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let rows: String = tracks.iter().enumerate().map(|(i, t)| format!(r##"<tr data-i="{i}">
        <td class="num"><span class="n">{disc}{track}</span><span class="rowplay" aria-hidden="true">{rowplay}</span></td>
        <td><div style="font-weight:600">{title}</div>{title_sub}{relpath}{source}</td>
        <td class="hide-sm muted">{artist}</td>
        <td class="hide-sm"><span class="badge">{fmt}</span>{nomb} {vb}{qb}</td>
        <td class="muted">{dur}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost" data-like="{id}" title="Like (syncs to Finamp/Plexamp)">{heart}</button><button class="btn sm ghost icon" data-tmenu="{i}" aria-label="More">{more}</button></td></tr>"##,
            // no track number in the tags (a YouTube download): the row's place in the album instead of a dash
            disc = if multi { format!("{}-", t.s("disc")) } else { String::new() }, track = if t.truthy("track") { t.s("track") } else { (i + 1).to_string() }, rowplay = icon("play"),
            title = esc(&ui::tr(t, "title")), title_sub = ui::tr_sub(t, "title"),
            relpath = if admin { format!(r##"<div class="small faint ellipsis" style="max-width:520px">{}</div>"##, esck(t, "relpath")) } else { String::new() },
            source = source_line(t), artist = ui::artist_links(t), fmt = format_text(t),
            nomb = if t.truthy("mb_trackid") { "" } else { r##" <span class="badge warn" title="No MusicBrainz id">?</span>"## },
            vb = verify_badge(t), dur = ui::fmt::dur(t.f("duration")), id = t.s("id"), heart = icon("heart"), more = icon("more"),
            qb = if t.s("quality_status") == "suspect" { format!(r##" <button class="badge warn" data-qfix="{i}" title="{} – tap to replace" style="cursor:pointer">Suspect quality</button>"##, esck(t, "quality_note")) } else { String::new() })).collect();
        dom::set_html(&view, &format!(r##"
    <div class="detail-head">
      <div class="cover">{cover}</div>
      <div class="grow">
        <div class="kicker">Album · In your library</div>
        <h1>{album}</h1>{album_sub}
        <div class="muted"><a href="#/artist-name/{artist_enc}" style="color:var(--text);font-weight:650">{artist}</a>
          {year} · {n} tracks · {dur} · {size}</div>
        {requested}
        <div class="chips mt">
          <span class="badge">{formats}</span>
          {genre}
          {tagged}
        </div>
        <div class="row wrap mt">
          <button class="btn primary" id="play-all">{play}Play</button>
          <button class="btn" id="shuffle-all">{shuffle}Shuffle</button>
          <button class="btn" id="album-share" title="Share link">{external}Share</button>
          {edit}
          <button class="btn icon" id="album-menu" aria-label="More options" title="More options">{more}</button>
        </div>
      </div>
    </div>
    <div class="card flat"><div class="table-wrap"><table class="table tracklist">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th class="hide-sm">Format</th><th>Time</th><th></th></tr></thead>
      <tbody>{rows}</tbody>
    </table></div></div>"##,
            cover = ui::art(&format!("/api/library/albums/{id}/cover{}", ui::cover_q(&a)), &a.s("album"), "big"),
            album = esc(&if a.truthy("album") { ui::tr(&a, "album") } else { "Unknown album".to_string() }), album_sub = ui::tr_sub(&a, "album"), artist_enc = enc(&a.s("albumartist")), artist = esc(&ui::tr_inline(&a, "albumartist")),
            year = if a.truthy("year") { format!(" · {}", esck(&a, "year")) } else { String::new() },
            n = tracks.len(), dur = ui::fmt::long_dur(a.f("duration")), size = ui::fmt::size(a.f("size")),
            requested = if history.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:4px">{history}</div>"##) },
            formats = esc(&formats),
            genre = if a.truthy("genre") { format!(r##"<span class="badge">{}</span>"##, esck(&a, "genre")) } else { String::new() },
            tagged = if untagged > 0 { format!(r##"<span class="badge warn">{untagged} track(s) not MusicBrainz-tagged</span>"##) } else { r##"<span class="badge ok">MusicBrainz tagged</span>"##.to_string() },
            play = icon("play"), shuffle = icon("shuffle"), external = icon("external"),
            edit = if admin { format!(r##"<button class="btn" id="album-edit" title="Change the album, artist and song names">{}Edit details</button>"##, icon("edit")) } else { String::new() },
            more = icon("more")));
        let queue_tracks = Rc::new(with_album_id(&tracks, &id));
        { let qt = queue_tracks.clone(); dom::on_sel(&view, "#play-all", "click", move |_| player::play_queue((*qt).clone(), 0, "album")); }
        { let qt = queue_tracks.clone(); dom::on_sel(&view, "#shuffle-all", "click", move |_| player::play_queue(shuffled(&qt), 0, "album")); }
        let reload: Rc<dyn Fn()> = Rc::new(hashchange);
        { let (id, a) = (id.clone(), a.clone()); dom::on_sel(&view, "#album-edit", "click", move |_| edit_modal(&id, &a)); }
        { let (id, a) = (id.clone(), a.clone()); dom::on_sel(&view, "#album-share", "click", move |_| dom::spawn(ui::share_album(id.clone(), a.clone()))); }
        {
            let menu_btn = need(&view, "#album-menu")?;
            let (mb, a, id, qt, reload) = (menu_btn.clone(), a.clone(), id.clone(), queue_tracks.clone(), reload.clone());
            dom::on(&menu_btn, "click", move |_| album_menu(&mb, &a, &id, &qt, admin, &reload));
        }
        let highlight: Rc<dyn Fn(Option<Value>)> = {
            let (view, tracks) = (view.clone(), tracks.clone());
            Rc::new(move |cur: Option<Value>| {
                for (i, tr) in dom::qsa_in(&view, "tbody tr").iter().enumerate() {
                    let on = match (&cur, tracks.get(i)) { (Some(c), Some(t)) => c.g("id") == t.g("id"), _ => false };
                    dom::toggle_class(tr, "playing", on);
                }
            })
        };
        { let h = highlight.clone(); ctx.on_leave(player::on_change(move |cur| h(cur))); }
        highlight(player::current());
        let liked: Rc<RefCell<HashSet<i64>>> = Rc::new(RefCell::new(
            api::get("/likes").await.map_err(|e| e.message)?.items().iter().filter_map(|v| v.as_i64()).collect()));
        let paint_likes: Rc<dyn Fn()> = {
            let (view, liked) = (view.clone(), liked.clone());
            Rc::new(move || {
                for b in dom::qsa_in(&view, "[data-like]") {
                    let on = dom::data(&b, "like").parse::<i64>().map(|i| liked.borrow().contains(&i)).unwrap_or(false);
                    dom::set_style(&b, "color", if on { "var(--accent-2)" } else { "" });
                    if let Some(svg) = dom::qs_in(&b, "svg") { let _ = svg.set_attribute("fill", if on { "currentColor" } else { "none" }); }
                }
            })
        };
        paint_likes();
        let tbody = need(&view, "tbody")?;
        {
            let (qt, reload) = (queue_tracks.clone(), reload.clone());
            dom::on(&tbody, "click", move |e| {
                let lb = dom::closest(e, "[data-like]");
                if let Some(lb) = &lb {
                    let tid: i64 = dom::data(lb, "like").parse().unwrap_or(0);
                    let (liked, paint) = (liked.clone(), paint_likes.clone());
                    dom::spawn(async move {
                        let has = liked.borrow().contains(&tid);
                        if has {
                            if api::del(&format!("/likes/{tid}")).await.is_err() { return; }
                            liked.borrow_mut().remove(&tid);
                        } else {
                            if api::post_empty(&format!("/likes/{tid}")).await.is_err() { return; }
                            liked.borrow_mut().insert(tid);
                        }
                        paint();
                    });
                }
                if let Some(tm) = dom::closest(e, "[data-tmenu]") {
                    if let Some(t) = dom::data(&tm, "tmenu").parse::<usize>().ok().and_then(|i| qt.get(i)) { track_menu(&tm, t, admin, Some(reload.clone())); }
                    return;
                }
                if let Some(qf) = dom::closest(e, "[data-qfix]") {
                    if let Some(t) = dom::data(&qf, "qfix").parse::<usize>().ok().and_then(|i| qt.get(i)).cloned() {
                        let reload = reload.clone();
                        dom::spawn(async move {
                            let info = api::get(&format!("/library/tracks/{}/info", t.i("id"))).await.ok();
                            let codes: Vec<String> = info.map(|i| i.o("quality").a("issues").iter().map(|x| x.s("code")).collect()).unwrap_or_default();
                            crate::pages::report::report_modal("track", t.g("id").clone(), &t.s("title"), crate::pages::report::reason_for(&codes), Some(Rc::new(move |_| reload())));
                        });
                    }
                    return;
                }
                if lb.is_none() && dom::closest(e, "a, button").is_none() {
                    if let Some(tr) = dom::closest(e, "tr[data-i]") {
                        player::play_queue((*qt).clone(), dom::data(&tr, "i").parse().unwrap_or(0), "album");
                    }
                }
            });
        }
        Ok(())
    })
}

/// The album page's ⋯ menu.
fn album_menu(anchor: &Element, a: &Value, id: &str, qt: &Rc<Vec<Value>>, admin: bool, reload: &Rc<dyn Fn()>) {
    let tracks = a.a("tracks");
    let ids: Vec<Value> = tracks.iter().map(|t| t.g("id").clone()).collect();
    let mut items: Vec<Option<MenuItem>> = Vec::new();
    { let qt = qt.clone(); items.push(MenuItem::new("Add to queue", &icon("list"), move || { player::enqueue((*qt).clone()); toast("Added to queue", "ok"); })); }
    { let qt = qt.clone(); items.push(MenuItem::new("Play next", &icon("next"), move || { player::play_next((*qt).clone()); toast("Plays next", "ok"); })); }
    { let ids: Vec<i64> = tracks.iter().map(|t| t.i("id")).collect(); items.push(MenuItem::new("Add to playlist", &icon("plus"), move || playlists::add_to_playlist_modal(ids.clone()))); }
    { let (id, a) = (id.to_string(), a.clone()); items.push(MenuItem::new("Share link", &icon("external"), move || dom::spawn(ui::share_album(id.clone(), a.clone())))); }
    if player::can_save_offline() {
        let qt = qt.clone();
        items.push(MenuItem::new("Save album for offline", &icon("download"), move || { player::save_offline((*qt).clone()); toast(&format!("Saving {} song(s) to this device…", qt.len()), "ok"); }));
    }
    if admin {
        let (ids, reload) = (ids.clone(), reload.clone());
        items.push(MenuItem::new("All songs are right (confirm)", &icon("check"), move || {
            let (ids, reload) = (ids.clone(), reload.clone());
            dom::spawn(async move {
                if api::post("/library/verify-manual", json!({ "track_ids": ids })).await.is_ok() { toast("Album confirmed", "ok"); reload(); }
            });
        }));
    }
    items.push(MenuItem::link("Download album (zip)", &icon("download"), &format!("/api/library/albums/{id}/download"), false));
    { let id = id.to_string(); items.push(MenuItem::new("Lyrics for all songs", &icon("music"), move || lyrics::fetch_album_lyrics(&id))); }
    // the catalog page of this album shows every song, with Request on the ones that aren't here yet
    {
        let (rgid, artist, album) = (a.s("mb_releasegroupid"), if a.truthy("albumartist") { a.s("albumartist") } else { a.s("artist") }, a.s("album"));
        items.push(MenuItem::new("Get the rest of the album", &icon("download"), move || {
            if !rgid.is_empty() { app::navigate(&format!("/album/{rgid}")); return; }
            let (artist, album) = (artist.clone(), album.clone());
            dom::spawn(async move {
                match api::get(&format!("/discover/album-lookup?artist={}&album={}", enc(&artist), enc(&album))).await {
                    Ok(r) if r.truthy("rgid") => app::navigate(&format!("/album/{}", r.s("rgid"))),
                    Ok(_) => toast("MusicBrainz doesn't know this album – look it up in the Downloader instead", "warn"),
                    Err(e) => toast(&e.message, "warn"),
                }
            });
        }));
    }
    if a.truthy("mb_releasegroupid") { items.push(MenuItem::link("Release info", &icon("disc"), &format!("#/album/{}", a.s("mb_releasegroupid")), false)); }
    { let (id, title) = (id.to_string(), a.s("album")); items.push(MenuItem::new("Report a problem", &icon("shield"), move || crate::pages::report::report_modal("album", json!(id), &title, "", Some(Rc::new(|_| app::navigate("/library")))))); }
    if ui::translate_mode() != "off" { let (album, reload) = (a.s("album"), reload.clone()); items.push(MenuItem::new("Edit translation", &icon("edit"), move || crate::pages::translate::edit_translation(album.clone(), "album", Some(reload.clone())))); }
    if admin {
        items.push(MenuItem::SEP);
        { let (id, a) = (id.to_string(), a.clone()); items.push(MenuItem::new("Tag / match metadata", &icon("tag"), move || match_modal(&id, &a))); }
        { let (id, a) = (id.to_string(), a.clone()); items.push(MenuItem::new("Edit details (album, artist, songs)", &icon("edit"), move || edit_modal(&id, &a))); }
        { let (id, a, reload) = (id.to_string(), a.clone(), reload.clone()); items.push(MenuItem::new("Change artwork…", &icon("disc"), move || artwork::artwork_modal(&id, &a, reload.clone()))); }
        {
            let (id, reload) = (id.to_string(), reload.clone());
            items.push(MenuItem::new("Find artwork automatically", &icon("refresh"), move || {
                let (id, reload) = (id.clone(), reload.clone());
                dom::spawn(async move {
                    match api::post_empty(&format!("/library/albums/{id}/artwork")).await {
                        Ok(r) => { toast(&format!("Artwork embedded in {} file(s)", r.s("embedded")), "ok"); reload(); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            }));
        }
        items.push(MenuItem::SEP);
        {
            let (id, a) = (id.to_string(), a.clone());
            items.push(MenuItem::danger("Delete album from disk", &icon("trash"), move || {
                let (id, a) = (id.clone(), a.clone());
                dom::spawn(async move {
                    if !confirm_dialog("Delete album", &format!("Permanently delete \"{}\" ({} files) from disk?", a.s("album"), a.a("tracks").len()), "Delete", true).await { return; }
                    if api::del(&format!("/library/albums/{id}")).await.is_err() { return; }
                    toast("Album deleted", "ok");
                    app::navigate("/library");
                });
            }));
        }
    }
    pop_menu(anchor, items);
}

// ---- matchModal ----

/// The "Tag this album" modal's shared state.
struct Mm { id: String, m: Modal, res: Element, provider: RefCell<String> }

fn match_help(provider: &str) -> &'static str {
    match provider {
        "musicbrainz" => "Full MusicBrainz tags (ids that Picard, Navidrome and Jellyfin use).",
        "deezer" => "Titles, track numbers, ISRCs, label, barcode and cover from Deezer – for releases MusicBrainz lacks or gets wrong.",
        "apple" => "Titles, track numbers, genre and a large cover from Apple Music.",
        "discogs" => "Vinyl, cassettes, bootlegs and old releases (needs a Discogs token in Settings).",
        "fingerprint" => "Ignores names: identifies each song by its audio, then finds the release most of them are on (needs an AcoustID key).",
        _ => "",
    }
}

/// `str.match(re)` → the captures (index 0 = the whole match).
fn re_match(pattern: &str, flags: &str, s: &str) -> Option<Vec<String>> {
    js_sys::RegExp::new(pattern, flags).exec(s).map(|arr| (0..arr.length()).map(|i| arr.get(i).as_string().unwrap_or_default()).collect())
}

impl Mm {
    /// `new FormData(#mq)` → (artist, album).
    fn form(&self) -> (String, String) {
        (dom::value_of(&self.m.el, "#mq [name=artist]"), dom::value_of(&self.m.el, "#mq [name=album]"))
    }

    fn paint_prov(&self) {
        let provider = self.provider.borrow().clone();
        for b in self.m.qsa("#mprov button") { dom::toggle_class(&b, "active", dom::data(&b, "p") == provider); }
        if let Some(h) = self.m.qs("#mprov-help") { dom::set_text(&h, match_help(&provider)); }
    }

    async fn search(self: Rc<Self>, artist: String, album: String) {
        let provider = self.provider.borrow().clone();
        dom::set_html(&self.res, &format!(r##"<div class="empty"><span class="spinner"></span>{}</div>"##,
            if provider == "fingerprint" { " Listening to every song… this takes a few seconds per song" } else { "" }));
        match api::get(&format!("/library/albums/{}/match?provider={provider}&artist={}&album={}", self.id, enc(&artist), enc(&album))).await {
            Ok(d) => {
                let cands = d.a("candidates");
                dom::set_html(&self.res, &if !cands.is_empty() {
                    format!(r##"<p class="small muted">Your files: {} tracks. Pick the matching release:</p>
        <div class="card flat"><div class="list">{}</div></div>"##, d.s("track_count"), cands.iter().map(|c| {
                        let mt = c.f("match");
                        format!(r##"
        <div class="list-item" style="cursor:pointer" data-rid="{rid}">
          <div class="thumb">{art}</div>
          <div class="grow"><div style="font-weight:600" class="ellipsis">{title} <span class="muted">· {artist}</span></div>
            <div class="small muted">{info}</div></div>
          {count}
          <span class="badge {cls}">{pct}%</span>
        </div>"##,
                            rid = esck(c, "id"), art = ui::art(&c.s("cover"), &c.s("title"), ""), title = esck(c, "title"), artist = esck(c, "artist"),
                            info = esc(&join_truthy(c, &["provider", "date", "country", "formats", "label", "status", "type"], " · ")),
                            count = if c.truthy("track_count") { format!(r##"<span class="badge {}">{} tracks</span>"##, if c.g("track_count") == d.g("track_count") { "ok" } else { "warn" }, c.s("track_count")) } else { String::new() },
                            cls = if mt > 0.85 { "ok" } else if mt > 0.6 { "warn" } else { "err" }, pct = round_pct(mt))
                    }).collect::<String>())
                } else { empty_state(&icon("search"), "No releases found", "Try editing the artist/album, or paste a release ID.") });
            }
            Err(e) => dom::set_html(&self.res, &format!(r##"<div class="error-box">{}</div>"##, esc(&e.message))),
        }
    }

    async fn preview(self: Rc<Self>, rid: String) {
        dom::set_html(&self.res, r##"<div class="empty"><span class="spinner"></span></div>"##);
        match api::get(&format!("/library/albums/{}/preview?release_id={}", self.id, enc(&rid))).await {
            Ok(pv) => {
                let r = pv.g("release").clone();
                let conf = pv.f("confidence");
                let provider = or_str(&r, "provider", "MusicBrainz");
                let rows: String = pv.a("rows").iter().map(|row| format!(r##"<tr>
            <td><div>{old_track}. {old_title}</div><div class="small faint">{old_artist}</div></td>
            <td>{new}</td>
            <td class="small mono faint hide-sm">{new_path}</td></tr>"##,
                    old_track = esc(&if row.is_nullish("old_track") { "–".to_string() } else { row.s("old_track") }), old_title = esck(row, "old_title"), old_artist = esck(row, "old_artist"),
                    new = if row.truthy("new_title") {
                        format!(r##"<div class="{}">{}{}. {}</div><div class="small faint">{}</div>"##,
                            if row.g("new_title") != row.g("old_title") { "diff-new" } else { "" },
                            if row.f("new_disc") > 1.0 { format!("{}-", row.s("new_disc")) } else { String::new() },
                            row.s("new_track"), esck(row, "new_title"), esck(row, "new_artist"))
                    } else { r##"<span class="badge err">no match – left untouched</span>"##.to_string() },
                    new_path = esck(row, "new_path"))).collect();
                dom::set_html(&self.res, &format!(r##"
        <div class="row" style="margin-bottom:12px">
          <div class="thumb lg">{art}</div>
          <div class="grow"><h3>{title}</h3><div class="small muted">{info}</div></div>
          <span class="badge {cls}">Confidence {pct}%</span>
        </div>
        <div class="card flat"><div class="table-wrap"><table class="table">
          <thead><tr><th>Current</th><th>→ {provider}</th><th class="hide-sm">New path</th></tr></thead>
          <tbody>{rows}</tbody>
        </table></div></div>
        <div class="row between mt">
          <label class="switch"><input type="checkbox" id="rename" checked><i></i>Rename & move files using naming template</label>
          <div class="row"><button class="btn" id="back">Back</button><button class="btn primary" id="apply">{check}Apply tags</button></div>
        </div>"##,
                    art = ui::art(&r.s("cover"), &r.s("title"), ""), title = esck(&r, "title"),
                    info = esc(&join_truthy(&r, &["artist", "date", "country", "label"], " · ")),
                    cls = if conf > 0.85 { "ok" } else if conf > 0.6 { "warn" } else { "err" }, pct = round_pct(conf),
                    provider = esc(&provider), check = icon("check")));
                {
                    let me = self.clone();
                    dom::on_sel(&self.res, "#back", "click", move |_| { let (artist, album) = me.form(); dom::spawn(me.clone().search(artist, album)); });
                }
                {
                    let me = self.clone();
                    dom::on_sel(&self.res, "#apply", "click", move |e| {
                        let target = dom::target_el(e);
                        let (me, rid, provider) = (me.clone(), rid.clone(), provider.clone());
                        dom::spawn(async move {
                            if let Some(b) = &target { dom::set_disabled(b, true); dom::set_html(b, r##"<span class="spinner"></span> Writing tags…"##); }
                            let rename = me.res.query_selector("#rename").ok().flatten().map(|c| dom::checked(&c)).unwrap_or(false);
                            match api::post(&format!("/library/albums/{}/retag", me.id), json!({ "release_id": rid, "rename": rename })).await {
                                Ok(out) => {
                                    toast(&format!("Album tagged from {provider}"), "ok");
                                    me.m.close();
                                    if out.truthy("album_id") && out.s("album_id") != me.id { app::navigate(&format!("/library/album/{}", out.s("album_id"))); }
                                    else { hashchange(); }
                                }
                                Err(err) => {
                                    toast(&err.message, "err");
                                    if let Some(b) = &target { dom::set_disabled(b, false); dom::set_text(b, "Apply tags"); }
                                }
                            }
                        });
                    });
                }
            }
            Err(e) => dom::set_html(&self.res, &format!(r##"<div class="error-box">{}</div>"##, esc(&e.message))),
        }
    }
}

fn match_modal(id: &str, a: &Value) {
    let provs = [("musicbrainz", "MusicBrainz"), ("deezer", "Deezer"), ("apple", "Apple Music"), ("discogs", "Discogs"), ("fingerprint", "By audio (AcoustID)")];
    let artist0 = if a.is_nullish("albumartist_orig") { a.s("albumartist") } else { a.s("albumartist_orig") };
    let body = format!(r##"<div class="tabs" id="mprov" style="margin-bottom:12px;max-width:100%">{}</div>
      <p class="small muted" id="mprov-help" style="margin:-4px 0 10px"></p>
      <form class="row wrap" id="mq">
        <input class="input grow" name="artist" value="{}" placeholder="Artist" style="max-width:260px">
        <input class="input grow" name="album" value="{}" placeholder="Album">
        <button class="btn" type="submit">{}Search</button>
        <input class="input" name="rid" placeholder="…or paste a MusicBrainz / Deezer / Apple / Discogs link" style="max-width:340px">
      </form>
      <div id="mres" class="mt"></div>"##,
        provs.iter().map(|(k, l)| format!(r##"<button type="button" data-p="{k}" class="{}">{l}</button>"##, if *k == "musicbrainz" { "active" } else { "" })).collect::<String>(),
        esc(&artist0), esck(a, "album"), icon("search"));
    let m = modal(ModalOpts::new("Tag this album", &body).wide(true));
    let Some(res) = m.qs("#mres") else { return };
    let mm = Rc::new(Mm { id: id.to_string(), m: m.clone(), res, provider: RefCell::new("musicbrainz".into()) });
    {
        let mm2 = mm.clone();
        dom::on_sel(&m.el, "#mprov", "click", move |e| {
            let Some(b) = dom::closest(e, "[data-p]") else { return };
            *mm2.provider.borrow_mut() = dom::data(&b, "p");
            mm2.paint_prov();
            let (artist, album) = mm2.form();
            dom::spawn(mm2.clone().search(artist, album));
        });
    }
    mm.paint_prov();
    {
        let mm2 = mm.clone();
        dom::on(&mm.res, "click", move |e| { if let Some(it) = dom::closest(e, "[data-rid]") { dom::spawn(mm2.clone().preview(dom::data(&it, "rid"))); } });
    }
    {
        let mm2 = mm.clone();
        dom::on_sel(&m.el, "#mq", "submit", move |e| {
            dom::prevent(e);
            let (artist, album) = mm2.form();
            let link = dom::value_of(&mm2.m.el, "#mq [name=rid]").trim().to_string();
            let mbid = re_match(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", "i", &link);
            let dz = re_match(r"deezer\.com/(?:\w+/)?album/(\d+)", "", &link);
            let ap = re_match(r"apple\.com/.*?/album/(?:[^/]+/)?(?:id)?(\d+)", "", &link);
            let dc = re_match(r"discogs\.com/(?:[^/]+/)?(release|master)/(\d+)", "", &link);
            let mm = mm2.clone();
            if let Some(m) = mbid { dom::spawn(mm.preview(m[0].clone())); }
            else if let Some(m) = dz { dom::spawn(mm.preview(format!("deezer:{}", m[1]))); }
            else if let Some(m) = ap { dom::spawn(mm.preview(format!("apple:{}", m[1]))); }
            else if let Some(m) = dc { dom::spawn(mm.preview(format!("discogs:{}{}", m[1].chars().next().map(|c| c.to_string()).unwrap_or_default(), m[2]))); }
            else { dom::spawn(mm.search(artist, album)); }
        });
    }
    dom::spawn(mm.clone().search(if artist0 == "Unknown Artist" { String::new() } else { artist0 }, a.s("album")));
}

// ---- editModal ----

fn edit_modal(id: &str, _shown: &Value) {
    let id = id.to_string();
    dom::spawn(async move {
        let Ok(a) = api::get(&format!("/library/albums/{id}")).await else { return };
        let tracks = a.a("tracks");
        let body = format!(r##"<p class="small muted" style="margin-top:0">Type what this album really is. The names are written into the files, and automatic tagging won't change them back.</p>
      <div class="form-grid">
        <label class="field"><span>Album artist</span><input class="input" name="albumartist" value="{}"></label>
        <label class="field"><span>Album</span><input class="input" name="album" value="{}"></label>
        <label class="field"><span>Year / date</span><input class="input" name="date" value="{}"></label>
        <label class="field"><span>Genre</span><input class="input" name="genre" value="{}"></label>
      </div>
      <div class="card flat mt"><div class="table-wrap"><table class="table"><thead><tr><th>Disc</th><th>#</th><th>Title</th><th>Artist</th></tr></thead><tbody>
      {}
      </tbody></table></div></div>
      <label class="switch mt"><input type="checkbox" id="ed-rename" checked><i></i>Move and rename the files to match (naming template)</label>"##,
            esck(&a, "albumartist"), esck(&a, "album"), esck(&a, "year"), esck(&a, "genre"),
            tracks.iter().map(|t| format!(r##"<tr data-id="{}">
        <td style="width:70px"><input class="input" name="discnumber" value="{}"></td>
        <td style="width:70px"><input class="input" name="tracknumber" value="{}"></td>
        <td><input class="input" name="title" value="{}"></td>
        <td><input class="input" name="artist" value="{}"></td></tr>"##,
                t.s("id"), esc(&or_str(t, "disc", "1")), esc(&or_str(t, "track", "")), esck(t, "title"), esck(t, "artist"))).collect::<String>());
        let m = modal(ModalOpts::new("Edit details", &body)
            .footer(r##"<button class="btn" data-close>Cancel</button><button class="btn primary" id="save">Save</button>"##)
            .wide(true));
        let (m2, a2, id2) = (m.clone(), a.clone(), id.clone());
        dom::on_sel(&m.el, "#save", "click", move |e| {
            let target = dom::target_el(e);
            let (m, a, id) = (m2.clone(), a2.clone(), id2.clone());
            dom::spawn(async move {
                if let Some(b) = &target { dom::set_disabled(b, true); }
                let tracks = a.a("tracks");
                let ids: Vec<Value> = tracks.iter().map(|t| t.g("id").clone()).collect();
                let result: Result<Value, ApiError> = async {
                    let vals = input_values(&m.el, ".form-grid input");
                    let mut out = api::post("/library/tags", json!({ "track_ids": ids, "values": Value::Object(vals) })).await?;
                    for tr in m.qsa("tbody tr") {
                        let tid = dom::data(&tr, "id");
                        let Some(t) = tracks.iter().find(|x| x.s("id") == tid) else { continue };
                        let v = input_values(&tr, "input");
                        let vs = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        if t.g("title").as_str() != Some(vs("title").as_str()) || t.g("artist").as_str() != Some(vs("artist").as_str())
                            || vs("tracknumber") != or_str(t, "track", "") || vs("discnumber") != or_str(t, "disc", "1") {
                            out = api::post("/library/tags", json!({ "track_ids": [t.g("id")], "values": Value::Object(v) })).await?;
                        }
                    }
                    if m.qs("#ed-rename").map(|c| dom::checked(&c)).unwrap_or(false) {
                        out = api::post("/library/tags", json!({ "track_ids": ids, "values": {}, "rename": true })).await?;
                    }
                    Ok(out)
                }.await;
                match result {
                    Ok(out) => {
                        toast("Saved", "ok");
                        m.close();
                        if out.truthy("album_id") && out.s("album_id") != id { app::navigate(&format!("/library/album/{}", out.s("album_id"))); }
                        else { hashchange(); }
                    }
                    Err(err) => { toast(&err.message, "err"); if let Some(b) = &target { dom::set_disabled(b, false); } }
                }
            });
        });
    });
}
