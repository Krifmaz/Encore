//! Import… on a folder (§4.14–§4.16 of the client shapes): the album folders in it, each with the release it was
//! matched to (the match review of Library → an album's ⋯ → Tag this album: the same candidate list and
//! confidence badges), and "Keep the files where they are", which adds the songs to the library where and as they
//! are: nothing is moved, renamed or retagged, and the folder becomes one of the library's extra folders.

use super::{logic, Root};
use crate::api;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{self, empty_state, esc, esck, icon, toast, Modal, ModalOpts};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

const KEEP_HELP: &str = "The songs join the library where and as they are: nothing is moved, renamed or retagged.";
const MOVE_HELP: &str = "Each album is tagged as the release it matches and moved into the library.";

/// The metadata sources the match review searches (the server's list).
const PROVIDERS: [(&str, &str); 5] = [("musicbrainz", "MusicBrainz"), ("deezer", "Deezer"), ("apple", "Apple Music"), ("discogs", "Discogs"), ("fingerprint", "By audio (AcoustID)")];

fn error_html(msg: &str) -> String {
    format!(r#"<div class="error-box row" style="align-items:flex-start"><span class="grow">{}</span>{}</div>"#, esc(msg), crate::details::copy_button(msg))
}

/// The import dialog's state.
struct Im {
    m: Modal,
    list: web_sys::Element,
    root: Root,
    /// the folder Import… was opened on, relative to the root
    path: String,
    keep: Cell<bool>,
    albums: RefCell<Vec<Value>>,
    /// album folders left out (their box unticked)
    off: RefCell<HashSet<String>>,
    /// album folders whose files are shown
    shown: RefCell<HashSet<String>>,
    seq: Cell<u32>,
    busy: Cell<bool>,
    on_done: Rc<dyn Fn()>,
}

/// The title of an album card: "Artist – Album", or its folder.
fn album_title(a: &Value) -> String {
    let t = [a.s("artist"), a.s("album")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" – ");
    if !t.is_empty() { t } else if a.s("folder").is_empty() { "This folder".into() } else { a.s("folder") }
}

/// One album folder: its box, what it is, what it matched (moving only), and its files.
pub fn album_html(a: &Value, keep: bool, on: bool, shown: bool) -> String {
    let folder = a.s("folder");
    let files = if a.has("files") { a.i("files") } else { a.a("rows").len() as i64 };
    let sub = format!("{} · {files} files", if folder.is_empty() { "This folder".to_string() } else { folder.clone() });
    let conf = logic::album_confidence(a);
    let release = a.g("release");
    let (matched, badge) = if keep { (String::new(), String::new()) } else if release.is_object() {
        let pct = (conf * 100.0).round() as i64;
        (format!(r#"<div class="small" style="margin-top:6px">Matches {}{}</div>"#, esc(&logic::release_line(release)), if pct > 0 { format!(" · {pct}% sure") } else { String::new() }),
         if pct > 0 { format!(r#"<span class="badge {}">{pct}%</span>"#, logic::conf_class(conf)) } else { String::new() })
    } else {
        (r#"<div class="small" style="margin-top:6px;color:var(--warn)">No match found: it is imported with its own tags.</div>"#.to_string(), String::new())
    };
    let rows = a.a("rows");
    let acts = if keep { String::new() } else {
        format!(r#"<div class="acts"><button type="button" class="btn sm" data-other>Other matches</button>{}</div>"#,
            if rows.is_empty() { String::new() } else { format!(r#"<button type="button" class="btn sm ghost" data-files>{}</button>"#, if shown { "Hide the files" } else { "Show the files" }) })
    };
    let table = if keep || !shown || rows.is_empty() { String::new() } else {
        format!(r#"<div class="table-wrap mt"><table class="table"><thead><tr><th>Current</th><th>New</th><th class="hide-sm">New path</th></tr></thead><tbody>{}</tbody></table></div>"#,
            rows.iter().map(|r| {
                let old = if r.truthy("old_title") { r.s("old_title") } else { logic::last_part(&r.s("path")).to_string() };
                let new = if r.truthy("new_title") {
                    let n = r.i("new_track");
                    format!(r#"<span class="{}">{}{}</span>"#, if r.g("new_title") != r.g("old_title") { "diff-new" } else { "" }, if n > 0 { format!("{n}. ") } else { String::new() }, esck(r, "new_title"))
                } else { r#"<span class="badge warn">(no match)</span>"#.to_string() };
                format!(r#"<tr><td>{}</td><td>{new}</td><td class="small mono faint hide-sm">{}</td></tr>"#, esc(&old), esck(r, "new_path"))
            }).collect::<String>())
    };
    format!(r#"<div class="card fm-album{off}" data-folder="{f}">
      <div class="row" style="gap:12px;align-items:flex-start">
        <input type="checkbox" data-on{checked} aria-label="Import this album" style="margin-top:3px;accent-color:var(--accent)">
        <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:650">{title}</div><div class="small muted ellipsis">{sub}</div>{matched}</div>{badge}
      </div>{acts}{table}</div>"#,
        off = if on { "" } else { " off" }, f = esc(&folder), checked = if on { " checked" } else { "" },
        title = esc(&album_title(a)), sub = esc(&sub))
}

/// Opens Import… for `path` (a folder) in `root`; `on_done` runs after an import.
pub fn import_modal(root: Root, path: String, on_done: Rc<dyn Fn()>) {
    // the library's own folder is already where songs live, and a read-only folder can't be moved from: both are
    // imported where they are unless the switch is turned off
    let keep = root.id == "library" || !root.writable;
    let body = format!(r#"<p class="small muted" style="margin-top:0">{}</p>
      <label class="switch"><input type="checkbox" id="im-keep"{}><i></i>Keep the files where they are</label>
      <p class="small muted" id="im-help" style="margin:8px 0 0"></p>
      <div id="im-list" class="mt"></div>"#,
        esc(&logic::place(&root.name, &path)), if keep { " checked" } else { "" });
    let m = ui::modal(ModalOpts::new("Import", &body)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="im-go" disabled>Import</button>"#)
        .wide(true));
    let Some(list) = m.qs("#im-list") else { return };
    let im = Rc::new(Im {
        m: m.clone(), list, root, path, keep: Cell::new(keep), albums: RefCell::new(vec![]), off: RefCell::new(HashSet::new()),
        shown: RefCell::new(HashSet::new()), seq: Cell::new(0), busy: Cell::new(false), on_done,
    });
    {
        let im2 = im.clone();
        dom::on_sel(&m.el, "#im-keep", "change", move |e| {
            let on = dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false);
            im2.keep.set(on);
            dom::spawn(im2.clone().load());
        });
    }
    {
        let im2 = im.clone();
        dom::on(&im.list, "change", move |e| {
            let Some(cb) = dom::closest(e, "[data-on]") else { return };
            let Some(card) = dom::closest_of(&cb, "[data-folder]") else { return };
            let f = dom::data(&card, "folder");
            if dom::checked(&cb) { im2.off.borrow_mut().remove(&f); } else { im2.off.borrow_mut().insert(f); }
            dom::toggle_class(&card, "off", !dom::checked(&cb));
            im2.paint_button();
        });
    }
    {
        let im2 = im.clone();
        dom::on(&im.list, "click", move |e| {
            let Some(card) = dom::closest(e, "[data-folder]") else { return };
            let f = dom::data(&card, "folder");
            if dom::closest(e, "[data-files]").is_some() {
                let open = !im2.shown.borrow().contains(&f);
                if open { im2.shown.borrow_mut().insert(f); } else { im2.shown.borrow_mut().remove(&f); }
                im2.paint();
            } else if dom::closest(e, "[data-other]").is_some() {
                let a = im2.albums.borrow().iter().find(|a| a.s("folder") == f).cloned();
                if let Some(a) = a { other_matches(im2.clone(), a); }
            }
        });
    }
    {
        let im2 = im.clone();
        dom::on_sel(&m.el, "#im-go", "click", move |_| dom::spawn(im2.clone().apply()));
    }
    dom::spawn(im.load());
}

impl Im {
    fn mode(&self) -> &'static str { if self.keep.get() { "in_place" } else { "move" } }

    fn chosen(&self) -> Vec<Value> {
        let off = self.off.borrow();
        self.albums.borrow().iter().filter(|a| !off.contains(&a.s("folder"))).cloned().collect()
    }

    fn paint_button(&self) {
        let Some(b) = self.m.qs("#im-go") else { return };
        let n = self.chosen().len();
        dom::set_text(&b, &if self.busy.get() { "Importing…".to_string() } else { logic::import_button(n) });
        dom::set_disabled(&b, n == 0 || self.busy.get());
    }

    fn paint(&self) {
        let keep = self.keep.get();
        let (off, shown) = (self.off.borrow(), self.shown.borrow());
        let html: String = self.albums.borrow().iter().map(|a| {
            let f = a.s("folder");
            album_html(a, keep, !off.contains(&f), shown.contains(&f))
        }).collect();
        dom::set_html(&self.list, &html);
        drop((off, shown));
        self.paint_button();
    }

    /// The preview for the switch's mode.
    async fn load(self: Rc<Self>) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let keep = self.keep.get();
        if let Some(h) = self.m.qs("#im-help") { dom::set_text(&h, if keep { KEEP_HELP } else { MOVE_HELP }); }
        self.albums.borrow_mut().clear();
        self.paint_button();
        dom::set_html(&self.list, &format!(r#"<div class="empty" style="padding:30px"><span class="spinner"></span><p class="muted">{}</p></div>"#,
            if keep { "Reading the folder…" } else { "Looking for matches…" }));
        let res = api::post("/files/import/preview", json!({ "root": self.root.id, "path": self.path, "mode": self.mode() })).await;
        if seq != self.seq.get() || !self.m.is_open() { return; }
        match res {
            Ok(d) => {
                let albums = if d.is_array() { d.items() } else { d.a("albums") };
                if albums.is_empty() {
                    dom::set_html(&self.list, &empty_state(&icon("music"), "No songs in this folder", ""));
                } else {
                    *self.albums.borrow_mut() = albums;
                    self.paint();
                }
            }
            Err(e) if e.status == 404 && e.message == "No songs in this folder" => dom::set_html(&self.list, &empty_state(&icon("music"), "No songs in this folder", "")),
            Err(e) => dom::set_html(&self.list, &error_html(if e.message.is_empty() { "Couldn't read the folder" } else { &e.message })),
        }
        self.paint_button();
    }

    async fn apply(self: Rc<Self>) {
        let chosen = self.chosen();
        if chosen.is_empty() || self.busy.get() { return; }
        self.busy.set(true);
        self.paint_button();
        let keep = self.keep.get();
        let albums: Vec<Value> = chosen.iter().map(|a| {
            let rel = a.g("release");
            json!({ "folder": a.s("folder"), "release_id": if keep || !rel.is_object() || rel.s("id").is_empty() { Value::Null } else { Value::String(rel.s("id")) } })
        }).collect();
        let res = api::post("/files/import/apply", json!({ "root": self.root.id, "path": self.path, "mode": self.mode(), "albums": albums })).await;
        self.busy.set(false);
        match res {
            Ok(a) => {
                let n = a.i("imported");
                let msg = if a.truthy("message") { a.s("message") } else if n == 1 { "Imported 1 song".to_string() } else { format!("Imported {n} songs") };
                toast(&msg, if a.a("failed").is_empty() { "ok" } else { "err" });
                self.m.close();
                (self.on_done)();
            }
            Err(e) => {
                toast(if e.message.is_empty() { "The import didn't work" } else { &e.message }, "err");
                self.paint_button();
            }
        }
    }

    /// An album's preview as the release picked in Other matches.
    fn replace(&self, a: Value) {
        let f = a.s("folder");
        if let Some(x) = self.albums.borrow_mut().iter_mut().find(|x| x.s("folder") == f) { *x = a; }
        self.paint();
    }
}

/// Other matches for one album folder: the match review's search over the metadata sources; a release picked is
/// previewed for that folder and takes the album's place in the import.
fn other_matches(im: Rc<Im>, album: Value) {
    let folder = album.s("folder");
    let files = album.i("files");
    let body = format!(r#"<div class="tabs" id="om-prov" style="margin-bottom:12px;max-width:100%">{}</div>
      <form class="row wrap" id="om-q">
        <input class="input grow" name="artist" value="{}" placeholder="Artist" style="max-width:260px">
        <input class="input grow" name="album" value="{}" placeholder="Album">
        <button class="btn" type="submit" id="om-go">{}Search</button>
      </form>
      <div id="om-res" class="mt"></div>"#,
        PROVIDERS.iter().map(|(k, l)| format!(r#"<button type="button" data-p="{k}" class="{}">{l}</button>"#, if *k == "musicbrainz" { "active" } else { "" })).collect::<String>(),
        esck(&album, "artist"), esck(&album, "album"), icon("search"));
    let m = ui::modal(ModalOpts::new("Other matches", &body).footer(r#"<button class="btn" data-close>Close</button>"#).wide(true));
    let Some(res) = m.qs("#om-res") else { return };
    let provider = Rc::new(RefCell::new("musicbrainz".to_string()));
    let seq = Rc::new(Cell::new(0u32));
    let search: Rc<dyn Fn()> = {
        let (m, res, provider, seq, im, folder) = (m.clone(), res.clone(), provider.clone(), seq, im.clone(), folder.clone());
        Rc::new(move || {
            let (m, res, provider, seq, im, folder) = (m.clone(), res.clone(), provider.borrow().clone(), seq.clone(), im.clone(), folder.clone());
            let n = seq.get() + 1;
            seq.set(n);
            let artist = dom::value_of(&m.el, "#om-q [name=artist]").trim().to_string();
            let album = dom::value_of(&m.el, "#om-q [name=album]").trim().to_string();
            dom::spawn(async move {
                let go = m.qs("#om-go");
                if let Some(b) = &go { dom::set_disabled(b, true); }
                dom::set_html(&res, r#"<div class="empty" style="padding:30px"><span class="spinner"></span><p class="muted">Searching…</p></div>"#);
                let r = api::post("/files/import/match", json!({ "root": im.root.id, "path": folder, "artist": artist, "album": album, "provider": provider })).await;
                if n != seq.get() || !m.is_open() { return; }
                if let Some(b) = &go { dom::set_disabled(b, false); }
                match r {
                    Ok(d) => {
                        let cands = d.a("candidates");
                        if cands.is_empty() { dom::set_html(&res, &empty_state(&icon("search"), "Nothing found", "")); return; }
                        dom::set_html(&res, &format!(r#"<div class="card flat"><div class="list">{}</div></div>"#, cands.iter().map(|c| {
                            let mt = logic::match_of(c);
                            let info = ["provider", "date", "country", "formats", "label", "status", "type"].iter().map(|k| c.s(k)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                            format!(r#"<div class="list-item" style="cursor:pointer" data-rid="{rid}" role="button" tabindex="0">
          <div class="thumb">{art}</div>
          <div class="grow"><div style="font-weight:600" class="ellipsis">{title} <span class="muted">· {artist}</span></div><div class="small muted">{info}</div></div>
          {count}<span class="badge {cls}">{pct}%</span></div>"#,
                                rid = esck(c, "id"), art = ui::art(&c.s("cover"), &c.s("title"), ""), title = esck(c, "title"), artist = esck(c, "artist"), info = esc(&info),
                                count = if c.truthy("track_count") { format!(r#"<span class="badge {}">{} songs</span>"#, if c.i("track_count") == files { "ok" } else { "warn" }, c.s("track_count")) } else { String::new() },
                                cls = logic::conf_class(mt), pct = (mt * 100.0).round() as i64)
                        }).collect::<String>()));
                    }
                    Err(e) => dom::set_html(&res, &error_html(&format!("The search didn't work: {}", e.message))),
                }
            });
        })
    };
    {
        let (provider, search, m2) = (provider, search.clone(), m.clone());
        dom::on_sel(&m.el, "#om-prov", "click", move |e| {
            let Some(b) = dom::closest(e, "[data-p]") else { return };
            *provider.borrow_mut() = dom::data(&b, "p");
            for x in m2.qsa("#om-prov [data-p]") { dom::toggle_class(&x, "active", dom::data(&x, "p") == *provider.borrow()); }
            search();
        });
    }
    {
        let search = search.clone();
        dom::on_sel(&m.el, "#om-q", "submit", move |e| { dom::prevent(e); search(); });
    }
    {
        let (m2, folder) = (m, folder);
        let pick = move |rid: String| {
            let (m, im, folder) = (m2.clone(), im.clone(), folder.clone());
            dom::spawn(async move {
                if let Some(r) = m.qs("#om-res") { dom::set_html(&r, r#"<div class="empty" style="padding:30px"><span class="spinner"></span></div>"#); }
                match api::post("/files/import/preview", json!({ "root": im.root.id, "path": folder, "mode": "move", "release_id": rid })).await {
                    Ok(d) => {
                        let albums = if d.is_array() { d.items() } else { d.a("albums") };
                        match albums.into_iter().next() {
                            Some(a) => { m.close(); im.replace(a); }
                            None => toast("Couldn't use that release", "err"),
                        }
                    }
                    Err(e) => {
                        toast(if e.message.is_empty() { "Couldn't use that release" } else { &e.message }, "err");
                        if let Some(r) = m.qs("#om-res") { dom::set_html(&r, ""); }
                    }
                }
            });
        };
        let pick = Rc::new(pick);
        let p2 = pick.clone();
        dom::on(&res, "click", move |e| { if let Some(it) = dom::closest(e, "[data-rid]") { p2(dom::data(&it, "rid")); } });
        dom::on(&res, "keydown", move |e| {
            if dom::key(e) != "Enter" { return; }
            if let Some(it) = dom::closest(e, "[data-rid]") { pick(dom::data(&it, "rid")); }
        });
    }
    search();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_card_says_what_it_matched() {
        let a = json!({"folder": "Gnarls Barkley/St. Elsewhere", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "files": 14,
            "release": {"id": "x", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006-05-08", "provider": "MusicBrainz", "score": 92},
            "confidence": 0.92,
            "rows": [{"path": "Gnarls Barkley/St. Elsewhere/02 crazy.flac", "old_title": "crazy", "new_title": "Crazy", "new_track": 2, "new_disc": 1, "new_path": "Gnarls Barkley/St. Elsewhere (2006)/02 Crazy.flac"},
                     {"path": "Gnarls Barkley/St. Elsewhere/99 bonus.flac", "old_title": "", "new_title": "", "new_path": ""}]});
        let h = album_html(&a, false, true, true);
        assert!(h.contains("Gnarls Barkley – St. Elsewhere"), "{h}");
        assert!(h.contains("Gnarls Barkley/St. Elsewhere · 14 files"), "{h}");
        assert!(h.contains("Matches Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz · 92% sure"), "{h}");
        assert!(h.contains(r#"<span class="badge ok">92%</span>"#), "{h}");
        assert!(h.contains("Hide the files") && h.contains("2. Crazy") && h.contains("99 bonus.flac") && h.contains("(no match)"), "{h}");
        assert!(h.contains(" checked"));
        // kept where it is: no match line, no files
        let k = album_html(&a, true, false, true);
        assert!(!k.contains("Matches") && !k.contains("Other matches") && !k.contains("<table"), "{k}");
        assert!(k.contains("fm-album off") && !k.contains(" checked"), "{k}");
        // nothing matched
        let n = album_html(&json!({"folder": "", "files": 3, "release": null, "rows": []}), false, true, false);
        assert!(n.contains("No match found: it is imported with its own tags.") && n.contains("This folder · 3 files"), "{n}");
        assert!(!n.contains("Show the files"), "{n}");
    }
}
