//! System → Files (admins, release 0.33): the admin file manager over the folders Encore may change – the
//! library, staging, the download folders and Trash. The routes and JSON are §2.4 and Part 4 of
//! docs/contracts/client-shapes-0.31-0.33.md; the words are its §5.7, the same as the phone's Files screen.
//!
//! - A tree of the roots, each folder's subfolders read when it is opened, with the free space of each root.
//! - The folder's list: sortable (the column heads, or "Sort: Name ↑"), 500 at a time ("Show more (N)"), picked
//!   with a click, Ctrl / ⌘-click, Shift-click or the check boxes. The keyboard: the arrows (with Shift),
//!   Enter opens, Backspace goes up a folder, Delete moves to trash (asked first), F2 renames, Ctrl+A picks all,
//!   a letter jumps to the next name starting with it.
//! - Breadcrumbs, and a box to type a path (the server refuses `..` and paths outside the roots, and says so).
//! - Files and folders dropped from the computer, or Upload files / Upload a folder; a panel shows the progress
//!   and has Stop (upload.rs).
//! - New folder, Rename, Move (a folder picker over the roots), Move to trash (dialogs.rs); the Trash with
//!   Restore, Delete forever and Empty trash.
//! - Edit tags for audio files (the Fix song dialog's form, tags.rs); Play and Edit song details… (the Fix song
//!   dialog itself) for songs the library has.
//! - Import… on a folder: the match review, with "Keep the files where they are" (import.rs).
//! - Every row has one ⋯ button, so a right-click opens the same menu (app.rs, the 0.26.8 rule).
//! - Errors are shown whole, with Copy.

mod dialogs;
mod import;
pub mod logic;
mod tags;
mod upload;

pub use upload::uploading;

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::ui::{self, confirm_dialog, empty_state, esc, fmt, icon, toast, MenuItem};
use futures::future::LocalBoxFuture;
use logic::Sel;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event};

/// Where the chosen sort is kept on this device.
const SORT_KEY: &str = "encore.filesSort";
/// The server's words for a folder reached through a link (§2.4).
const THROUGH_LINK: &str = "This folder is reached through a link, so Encore doesn't change anything in it.";

/// A place the file manager may touch (`GET /api/files/roots`).
#[derive(Clone, Debug)]
pub struct Root {
    pub id: String,
    pub name: String,
    pub path: String,
    pub writable: bool,
    pub free: f64,
}

impl Root {
    fn from(v: &Value) -> Option<Root> {
        let id = v.s("id");
        if id.is_empty() { return None; }
        let name = if v.s("name").is_empty() { id.clone() } else { v.s("name") };
        Some(Root { id, name, path: v.s("path"), writable: !v.has("writable") || v.truthy("writable"), free: v.f("free_bytes") })
    }
    pub fn is_trash(&self) -> bool { self.id == "trash" }
}

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

// ---- icons (the set in ui.rs has no folder or file) --------------------------------------------------------------

fn svg(d: &str) -> String {
    format!(r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{d}</svg>"#)
}
fn svg_folder() -> String { svg(r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#) }
fn svg_file() -> String { svg(r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#) }
fn svg_image() -> String { svg(r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-5-5L5 21"/>"#) }
fn svg_upload() -> String { svg(r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m17 8-5-5-5 5M12 3v12"/>"#) }
fn svg_folder_plus() -> String { svg(r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 10v6M9 13h6"/>"#) }
fn svg_move() -> String { svg(r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M8 13h7M12 10l3 3-3 3"/>"#) }
fn svg_restore() -> String { svg(r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>"#) }
fn svg_up() -> String { svg(r#"<path d="M12 19V5M5 12l7-7 7 7"/>"#) }
fn svg_sort() -> String { svg(r#"<path d="M7 4v16M3 8l4-4 4 4M17 20V4M13 16l4 4 4-4"/>"#) }
fn svg_blank() -> String { svg("") }

// ---- the page's look ------------------------------------------------------------------------------------------------

const CSS: &str = r#"
.fm { display: grid; grid-template-columns: 236px minmax(0, 1fr); gap: 16px; align-items: start; margin-top: 16px; }
.fm-tree { padding: 8px 6px; position: sticky; top: 20px; max-height: calc(100vh - 140px); overflow: auto; }
.fm-tree-head { padding: 2px 8px 10px; }
.fm-node-row { display: flex; align-items: center; border-radius: 8px; min-width: 0; }
.fm-node-row:hover { background: var(--panel-2); }
.fm-node-row.on { background: rgba(var(--accent-rgb), .16); }
.fm-tw { flex: none; width: 22px; height: 28px; padding: 0; border: 0; background: transparent; color: var(--faint); cursor: pointer; display: inline-flex; align-items: center; justify-content: center; }
.fm-tw svg { width: 14px; height: 14px; transform: rotate(-90deg); transition: transform .12s; }
.fm-node.open > .fm-node-row > .fm-tw svg { transform: none; }
.fm-tw.leaf { visibility: hidden; }
.fm-nm { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; padding: 5px 8px 5px 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 13.5px; text-align: left; cursor: pointer; }
.fm-nm > svg { width: 16px; height: 16px; flex: none; color: var(--accent); }
.fm-nm .fm-sub { display: block; font-size: 11.5px; font-weight: 400; color: var(--faint); }
.fm-node.root > .fm-node-row .fm-nm { font-weight: 650; }
.fm-node:not(.open) > .fm-kids { display: none; }
.fm-msg { font-size: 12px; color: var(--muted); padding: 4px 8px; }
.fm-main { min-width: 0; }
.fm-crumbs { display: flex; flex-wrap: wrap; align-items: center; gap: 2px; min-height: 36px; }
.fm-crumbs .crumb { border: 0; background: transparent; padding: 4px 6px; border-radius: 6px; color: var(--accent); font: inherit; font-size: 16px; font-weight: 650; cursor: pointer; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fm-crumbs .crumb:hover { background: var(--panel-2); }
.fm-crumbs .crumb.cur { color: var(--text); cursor: default; background: transparent; }
.fm-crumbs .sep { color: var(--faint); }
.fm-crumbs form { flex: 1; display: flex; gap: 8px; align-items: center; min-width: 220px; }
.fm-info { display: flex; gap: 8px; align-items: center; margin: 2px 6px 0; min-width: 0; }
.fm-info .mono { flex: 0 1 auto; min-width: 0; }
.fm-info .fm-free, .fm-info .badge { flex: none; white-space: nowrap; }
.fm-tools { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin: 12px 0 10px; min-height: 32px; }
.fm-tools .count { font-weight: 650; margin-right: 4px; }
.fm-note { margin: 0 0 10px; }
.fm-wrap { position: relative; }
.fm-wrap.dropping { outline: 2px dashed var(--accent); outline-offset: 3px; }
.fm-hint { display: none; position: absolute; inset: 0; z-index: 2; align-items: center; justify-content: center; text-align: center; padding: 20px; background: rgba(var(--accent-rgb), .10); font-weight: 650; pointer-events: none; border-radius: inherit; }
.fm-wrap.dropping .fm-hint { display: flex; }
.fm-list { outline: none; min-height: 120px; }
.fm-list:focus-visible { box-shadow: inset 0 0 0 2px rgba(var(--accent-rgb), .45); border-radius: inherit; }
.fm-head, .fm-row { display: grid; grid-template-columns: 18px 22px minmax(0, 1fr) 64px 108px 34px; gap: 10px; align-items: center; padding: 6px 8px 6px 12px; }
.fm-head { border-bottom: 1px solid var(--border); }
.fm-head:empty { display: none; }
.fm-head button { border: 0; background: transparent; padding: 4px 0; color: var(--faint); font: inherit; font-size: 11.5px; font-weight: 650; letter-spacing: .06em; text-transform: uppercase; text-align: left; cursor: pointer; white-space: nowrap; }
.fm-head button.on { color: var(--text); }
.fm-head .r, .fm-row .r { text-align: right; }
.fm-row { border-bottom: 1px solid var(--border); user-select: none; -webkit-user-select: none; min-width: 0; }
.fm-row:last-child { border-bottom: 0; }
.fm-row:hover { background: var(--panel-2); }
.fm-row.sel { background: rgba(var(--accent-rgb), .14); }
.fm-row.cur { box-shadow: inset 3px 0 0 var(--accent); }
.fm-row.drop { background: rgba(var(--accent-rgb), .26); }
.fm-row .fm-ic { display: inline-flex; color: var(--muted); }
.fm-row .fm-ic svg { width: 20px; height: 20px; }
.fm-row.dir .fm-ic { color: var(--accent); }
.fm-row .nm { font-size: 14px; }
.fm-row.dir .nm { font-weight: 650; }
.fm-row .fm-check { margin: 0; width: 16px; height: 16px; accent-color: var(--accent); opacity: .3; cursor: pointer; }
.fm-row:hover .fm-check, .fm-row.sel .fm-check, .fm-row .fm-check:focus-visible { opacity: 1; }
.fm-row .r { font-size: 12.5px; color: var(--muted); white-space: nowrap; }
.fm-more { display: flex; justify-content: center; margin-top: 12px; }
.fm-more:empty { display: none; }
.fm-up { position: fixed; z-index: 95; left: 50%; transform: translateX(-50%); bottom: calc(var(--player) + 18px); width: min(460px, calc(100vw - 24px)); background: var(--panel); border: 1px solid var(--border); border-radius: 14px; box-shadow: var(--shadow); padding: 12px 14px; }
.fm-up .progress { margin: 10px 0 6px; }
.fm-album { padding: 12px 14px; }
.fm-album + .fm-album { margin-top: 10px; }
.fm-album.off { opacity: .55; }
.fm-album .acts { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }
.fm-radio { display: flex; align-items: center; gap: 10px; padding: 6px 0; cursor: pointer; }
.fm-radio input { accent-color: var(--accent); margin: 0; }
.fp-row { width: 100%; border: 0; border-bottom: 1px solid var(--border); background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.fp-row:disabled { opacity: .45; cursor: not-allowed; }
.fp-row svg { width: 18px; height: 18px; flex: none; color: var(--accent); }
@media (max-width: 1100px) { .fm { grid-template-columns: minmax(0, 1fr); } .fm-tree { position: static; max-height: 240px; } }
@media (max-width: 760px) {
  .fm-head { display: none; }
  .fm-row { grid-template-columns: 18px 22px minmax(0, 1fr) 34px; }
  .fm-row .r { display: none; }
  .fm-row .fm-check { opacity: 1; }
  .fm-tree { max-height: 190px; }
}
"#;

fn ensure_css() {
    if dom::by_id("fm-css").is_some() { return; }
    let s = dom::create("style");
    s.set_id("fm-css");
    s.set_text_content(Some(CSS));
    if let Some(h) = dom::document().head() { let _ = h.append_child(&s); }
}

/// An action's error toast: the server's words (with Copy), or "That didn't work" when it gave none.
fn toast_error(e: &api::ApiError) {
    toast(if e.message.is_empty() { "That didn't work" } else { &e.message }, "err");
}

fn error_html(msg: &str) -> String {
    format!(r#"<div class="error-box row" role="alert" style="align-items:flex-start"><span class="grow">{}</span>{}</div>"#, esc(msg), crate::details::copy_button(msg))
}

/// Whether a dialog or a ⋯ menu is open (the list's keys are theirs then).
fn overlay_open() -> bool {
    dom::by_id("modal-root").is_some_and(|m| m.child_element_count() > 0) || dom::qs(".menu-pop").is_some()
}

/// `el.scrollIntoView({block: 'nearest'})`.
fn scroll_near(el: &Element) {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"block".into(), &"nearest".into());
    if let Ok(f) = js_sys::Reflect::get(el, &"scrollIntoView".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) { let _ = f.call1(el, &opts); }
}

/// `el.focus({preventScroll: true})`.
fn focus_still(el: &Element) {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"preventScroll".into(), &JsValue::TRUE);
    if let Ok(f) = js_sys::Reflect::get(el, &"focus".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) { let _ = f.call1(el, &opts); }
}

// ---- the page --------------------------------------------------------------------------------------------------------

/// Everything the page's handlers share.
struct Fm {
    view: Element,
    tree: Element,
    crumbs: Element,
    info: Element,
    tools: Element,
    note: Element,
    wrap: Element,
    hint: Element,
    head: Element,
    list: Element,
    more: Element,
    roots: RefCell<Vec<Root>>,
    root: RefCell<String>,
    path: RefCell<String>,
    /// the folder open was reached through a link: nothing in it is changed
    via_link: Cell<bool>,
    /// the folder's entries (§2.4), or the trash's items (§4.8)
    rows: RefCell<Vec<Value>>,
    total: Cell<usize>,
    /// the folder answered (false while it loads, and after an error)
    ready: Cell<bool>,
    sel: RefCell<Sel>,
    sort: RefCell<(String, String)>,
    seq: Cell<u32>,
    hint_text: RefCell<String>,
    /// the list has had the focus: it gets it back after a dialog
    used: Cell<bool>,
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        ensure_css();
        dom::set_html(&view, r#"
    <div><h1>Files</h1><div class="small muted">Browse, upload, move, rename and import in the library and download folders</div></div>
    <div class="fm">
      <aside class="card flat fm-tree" aria-label="Folders">
        <div class="small muted fm-tree-head">The folders Encore may change. Deleted files go to Trash first.</div>
        <div id="fm-roots" role="tree"><div class="empty" style="padding:24px"><span class="spinner"></span></div></div>
      </aside>
      <section class="fm-main">
        <nav class="fm-crumbs" id="fm-crumbs" aria-label="Folder"></nav>
        <div class="small muted fm-info" id="fm-info"></div>
        <div class="fm-tools" id="fm-tools"></div>
        <div class="small muted fm-note" id="fm-note" hidden></div>
        <div class="card flat fm-wrap" id="fm-wrap">
          <div class="fm-hint" id="fm-hint"></div>
          <div class="fm-head" id="fm-head"></div>
          <div class="fm-list" id="fm-list" tabindex="0" role="grid" aria-label="Files" aria-multiselectable="true"></div>
        </div>
        <div class="fm-more" id="fm-more"></div>
        <input type="file" id="fm-pick-files" multiple hidden>
        <input type="file" id="fm-pick-dir" multiple webkitdirectory hidden>
      </section>
    </div>"#);
        let need = |sel: &str| dom::qs_in(&view, sel).ok_or_else(|| format!("{sel} is missing"));
        let (tree, crumbs, info, tools, note) = (need("#fm-roots")?, need("#fm-crumbs")?, need("#fm-info")?, need("#fm-tools")?, need("#fm-note")?);
        let (wrap, hint, head, list, more) = (need("#fm-wrap")?, need("#fm-hint")?, need("#fm-head")?, need("#fm-list")?, need("#fm-more")?);
        let roots = match api::get("/files/roots").await {
            Ok(d) => { let l = if d.is_array() { d.items() } else { d.a("roots") }; l.iter().filter_map(Root::from).collect::<Vec<_>>() }
            Err(e) => {
                let msg = format!("Couldn't load the folders: {}", e.message);
                dom::set_html(&tree, &format!(r#"<div style="padding:8px">{}</div>"#, error_html(&msg)));
                dom::set_html(&list, &format!(r#"<div style="padding:16px">{}</div>"#, error_html(&msg)));
                return Ok(());
            }
        };
        let saved: Value = dom::ls_get(SORT_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let sort = if logic::SORTS.iter().any(|(k, _)| *k == saved.s("sort")) {
            (saved.s("sort"), if saved.s("dir") == "desc" { "desc".to_string() } else { "asc".to_string() })
        } else { ("name".to_string(), "asc".to_string()) };
        let fm = Rc::new(Fm {
            view: view.clone(), tree, crumbs, info, tools, note, wrap, hint, head, list, more,
            roots: RefCell::new(roots), root: RefCell::new(String::new()), path: RefCell::new(String::new()), via_link: Cell::new(false),
            rows: RefCell::new(vec![]), total: Cell::new(0), ready: Cell::new(false), sel: RefCell::new(Sel::new(0)), sort: RefCell::new(sort),
            seq: Cell::new(0), hint_text: RefCell::new(String::new()), used: Cell::new(false),
        });
        fm.paint_roots();
        fm.wire();
        // #/settings?s=files&root=staging&path=Album opens that folder (the page keeps the address up to date)
        let (qr, qp) = if ctx.q("s") == "files" { (ctx.q("root"), ctx.q("path")) } else { (String::new(), String::new()) };
        let start = {
            let roots = fm.roots.borrow();
            match roots.iter().find(|r| r.id == qr && !qr.is_empty()) {
                Some(r) => Some((r.id.clone(), qp)),
                None => roots.iter().find(|r| !r.is_trash()).or(roots.first()).map(|r| (r.id.clone(), String::new())),
            }
        };
        match start {
            Some((r, p)) => fm.go(&r, &p),
            None => dom::set_html(&fm.list, &empty_state(&svg_folder(), "No folders here", "")),
        }
        Ok(())
    })
}

/// A row's key: an entry's name, a trash item's id.
fn key_of(e: &Value, trash: bool) -> String { if trash { e.s("id") } else { e.s("name") } }

impl Fm {
    fn cur(&self) -> (String, String) { (self.root.borrow().clone(), self.path.borrow().clone()) }
    fn root_info(&self) -> Option<Root> { let id = self.root.borrow().clone(); self.roots.borrow().iter().find(|r| r.id == id).cloned() }
    fn root_name(&self, id: &str) -> String { self.roots.borrow().iter().find(|r| r.id == id).map(|r| r.name.clone()).unwrap_or_else(|| id.to_string()) }
    fn in_trash(&self) -> bool { *self.root.borrow() == "trash" }
    /// Whether this folder may be written to: its root is writable, it isn't Trash, it isn't reached through a link.
    fn can_write(&self) -> bool { self.ready.get() && !self.in_trash() && !self.via_link.get() && self.root_info().is_some_and(|r| r.writable) }
    /// Why nothing may be written here, in the server's words.
    fn why_not(&self) -> &'static str {
        if self.in_trash() { "Files go to the trash with Move to trash" } else if self.via_link.get() { THROUGH_LINK } else { "Read-only" }
    }
    fn row(&self, i: usize) -> Option<Value> { self.rows.borrow().get(i).cloned() }
    fn keys(&self) -> Vec<String> { let t = self.in_trash(); self.rows.borrow().iter().map(|e| key_of(e, t)).collect() }
    fn picked(&self) -> Vec<usize> { self.sel.borrow().picked() }
    fn picked_keys(&self) -> Vec<String> { let k = self.keys(); self.picked().into_iter().filter_map(|i| k.get(i).cloned()).collect() }
    fn place(&self, path: &str) -> String { logic::place(&self.root_name(&self.root.borrow()), path) }

    // ---- moving around ----

    /// Opens a folder (or Trash): the address, the crumbs, the tree and the list follow.
    fn go(self: &Rc<Self>, root: &str, path: &str) {
        *self.root.borrow_mut() = root.to_string();
        *self.path.borrow_mut() = path.to_string();
        self.via_link.set(false);
        self.ready.set(false);
        self.rows.borrow_mut().clear();
        *self.sel.borrow_mut() = Sel::new(0);
        self.save_url();
        self.paint_crumbs();
        self.paint_info();
        self.paint_head();
        self.paint_tools();
        dom::set_html(&self.list, r#"<div class="empty" style="padding:40px"><span class="spinner"></span></div>"#);
        dom::set_html(&self.more, "");
        self.tree_mark();
        dom::spawn(self.clone().tree_reveal());
        dom::spawn(self.clone().load(vec![]));
    }

    fn up(self: &Rc<Self>) {
        let (r, p) = self.cur();
        if r != "trash" && !p.is_empty() { self.go(&r, &logic::parent_of(&p)); }
    }

    /// The same folder again, with `keep` (names) picked.
    fn reload(self: &Rc<Self>, keep: Vec<String>) { dom::spawn(self.clone().load(keep)); }
    fn reload_same(self: &Rc<Self>) { let k = self.picked_keys(); self.reload(k); }

    /// `#/settings?s=files&root=…&path=…`, so a reload or a shared address opens the same folder.
    fn save_url(&self) {
        if !self.view.is_connected() { return; }
        let (r, p) = self.cur();
        let mut url = format!("#/settings?s=files&root={}", enc(&r));
        if !p.is_empty() { url.push_str(&format!("&path={}", enc(&p))); }
        if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&url)); }
    }

    async fn load(self: Rc<Self>, keep: Vec<String>) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let (root, path) = self.cur();
        let trash = root == "trash";
        let res = if trash { api::get("/files/trash").await } else {
            let (k, d) = self.sort.borrow().clone();
            api::get(&format!("/files/list?root={}&path={}&sort={k}&dir={d}&offset=0&limit=500", enc(&root), enc(&path))).await
        };
        if seq != self.seq.get() || !self.view.is_connected() { return; }
        match res {
            Ok(d) => {
                let rows = if trash { if d.is_array() { d.items() } else { d.a("items") } } else { d.a("entries") };
                self.total.set(if trash { rows.len() } else { (d.i("total").max(0) as usize).max(rows.len()) });
                self.via_link.set(!trash && d.truthy("via_link"));
                let keys: Vec<String> = rows.iter().map(|e| key_of(e, trash)).collect();
                *self.sel.borrow_mut() = Sel::keep(&keep, &keys);
                *self.rows.borrow_mut() = rows;
                self.ready.set(true);
                self.paint_info();
                self.paint_rows();
                // after a dialog the keys work on the list again
                let active = dom::document().active_element();
                if self.used.get() && (active.is_none() || active.is_some_and(|a| a.tag_name() == "BODY")) { focus_still(&self.list); }
            }
            Err(e) => {
                self.rows.borrow_mut().clear();
                *self.sel.borrow_mut() = Sel::new(0);
                self.ready.set(false);
                self.via_link.set(false);
                let msg = if !e.message.is_empty() { e.message } else if trash { "Couldn't open the trash".into() } else { "Couldn't open the folder".into() };
                self.paint_info();
                let back = if path.is_empty() || trash { String::new() } else {
                    format!(r#"<div class="row mt"><button type="button" class="btn sm" data-top>{}{}</button></div>"#, svg_up(), esc(&self.root_name(&root)))
                };
                dom::set_html(&self.list, &format!(r#"<div style="padding:16px">{}{back}</div>"#, error_html(&msg)));
                dom::set_html(&self.more, "");
                self.paint_tools();
            }
        }
    }

    /// "Show more (N)": the next 500.
    async fn load_more(self: Rc<Self>) {
        let (root, path) = self.cur();
        let offset = self.rows.borrow().len();
        let (k, d) = self.sort.borrow().clone();
        let seq = self.seq.get();
        let res = api::get(&format!("/files/list?root={}&path={}&sort={k}&dir={d}&offset={offset}&limit=500", enc(&root), enc(&path))).await;
        if seq != self.seq.get() || !self.view.is_connected() { return; }
        match res {
            Ok(d) => {
                let have = self.keys();
                let new: Vec<Value> = d.a("entries").into_iter().filter(|e| !have.contains(&e.s("name"))).collect();
                self.sel.borrow_mut().on.extend(std::iter::repeat_n(false, new.len()));
                self.rows.borrow_mut().extend(new);
                self.total.set((d.i("total").max(0) as usize).max(self.rows.borrow().len()));
                self.paint_rows();
            }
            Err(e) => toast(if e.message.is_empty() { "Couldn't load more" } else { &e.message }, "err"),
        }
    }

    // ---- painting ----

    fn paint_crumbs(&self) {
        let (r, p) = self.cur();
        let parts = logic::crumbs(&p);
        let mut html = format!(r#"<button type="button" class="crumb{}" data-crumb="">{}</button>"#, if parts.is_empty() { " cur" } else { "" }, esc(&self.root_name(&r)));
        for (i, (label, at)) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            html.push_str(&format!(r#"<span class="sep">›</span><button type="button" class="crumb{}" data-crumb="{}"{}>{}</button>"#,
                if last { " cur" } else { "" }, esc(at), if last { r#" aria-current="page""# } else { "" }, esc(label)));
        }
        if r != "trash" {
            html.push_str(&format!(r#"<button type="button" class="btn sm ghost icon" id="fm-path-edit" title="Type a path" aria-label="Type a path">{}</button>"#, icon("edit")));
        }
        dom::set_html(&self.crumbs, &html);
    }

    /// The path box: the folder's path, editable; Enter opens what was typed (the server says no to `..`).
    fn edit_path(&self) {
        let r = self.root_info().map(|r| r.name).unwrap_or_default();
        dom::set_html(&self.crumbs, &format!(r#"<form id="fm-path-form" autocomplete="off">
            <span class="small muted" style="white-space:nowrap">{} /</span>
            <input class="input grow mono" id="fm-path" value="{}" placeholder="Artist/Album" spellcheck="false" aria-label="Path">
            <button class="btn sm primary" type="submit">Open</button><button class="btn sm" type="button" id="fm-path-cancel">Cancel</button></form>"#,
            esc(&r), esc(&self.path.borrow())));
        if let Some(i) = dom::qs_in(&self.crumbs, "#fm-path") {
            dom::focus(&i);
            if let Some(h) = i.dyn_ref::<web_sys::HtmlInputElement>() { h.select(); }
        }
    }

    fn paint_info(&self) {
        let Some(r) = self.root_info() else { dom::set_html(&self.info, ""); return };
        let p = self.path.borrow().clone();
        // the folder's own path once the server has shown it, the root's until then (a refused path isn't repeated)
        let full = if p.is_empty() || !self.ready.get() { r.path.clone() } else { format!("{}/{p}", r.path.trim_end_matches('/')) };
        let mut html = format!(r#"<span class="mono ellipsis" title="{f}">{f}</span>"#, f = esc(&full));
        if r.free > 0.0 { html.push_str(&format!(r#"<span class="fm-free">· {} free</span>"#, fmt::size(r.free))); }
        if !r.writable && !r.is_trash() { html.push_str(r#"<span class="badge">Read-only</span>"#); }
        dom::set_html(&self.info, &html);
        dom::set_text(&self.note, THROUGH_LINK);
        dom::hide(&self.note, !self.via_link.get());
    }

    fn paint_head(&self) {
        if self.in_trash() { dom::set_html(&self.head, ""); return; }
        let (k, d) = self.sort.borrow().clone();
        let cols: String = logic::SORTS.iter().map(|(key, label)| {
            let on = *key == k;
            let arrow = if on { if d == "desc" { " ↓" } else { " ↑" } } else { "" };
            format!(r#"<button type="button" data-sort="{key}" class="{}{}" title="{}">{label}{arrow}</button>"#,
                if on { "on" } else { "" }, if *key == "name" { "" } else { " r" }, esc(logic::order_text(key, if on { &d } else { logic::first_dir(key) })))
        }).collect();
        dom::set_html(&self.head, &format!("<span></span><span></span>{cols}<span></span>"));
    }

    fn row_html(&self, i: usize, e: &Value, trash: bool) -> String {
        let (cls, ic, name, line, size, date) = if trash {
            let dir = e.s("kind") == "dir";
            let mut line = vec![logic::trash_from(&self.root_name(&e.s("root")), &e.s("path"))];
            if e.truthy("by") { line.push(format!("by {}", e.s("by"))); }
            let name = if e.s("name").is_empty() { logic::last_part(&e.s("path")).to_string() } else { e.s("name") };
            ("", if dir { svg_folder() } else { svg_file() }, name, line.join(" · "), if e.f("size") > 0.0 { fmt::size(e.f("size")) } else { String::new() }, fmt::day(e.f("at")))
        } else {
            let dir = logic::is_dir(e);
            let ic = if logic::is_link(e) { icon("link") } else if dir { svg_folder() } else if logic::is_audio(e) { icon("music") } else if e.truthy("image") { svg_image() } else { svg_file() };
            (if dir { " dir" } else { "" }, ic, e.s("name"), logic::entry_line(e), if dir { String::new() } else { fmt::size(e.f("size")) }, fmt::day(e.f("mtime")))
        };
        let line = if line.is_empty() { String::new() } else { format!(r#"<div class="small muted ellipsis" title="{l}">{l}</div>"#, l = esc(&line)) };
        format!(r#"<div class="fm-row{cls}" data-i="{i}" role="row" aria-selected="false">
      <input type="checkbox" class="fm-check" tabindex="-1" aria-label="Select {n}">
      <span class="fm-ic">{ic}</span>
      <div style="min-width:0"><div class="nm ellipsis" title="{n}">{n}</div>{line}</div>
      <div class="r">{size}</div><div class="r">{date}</div>
      <button type="button" class="btn sm ghost icon" data-act="menu" aria-label="More" title="More">{more}</button></div>"#,
            n = esc(&name), more = icon("more"))
    }

    fn paint_rows(&self) {
        let trash = self.in_trash();
        let rows = self.rows.borrow().clone();
        if rows.is_empty() {
            dom::set_html(&self.list, &if trash {
                empty_state(&icon("trash"), "The trash is empty", "Files you move to trash wait here until you delete them forever.")
            } else {
                empty_state(&svg_folder(), "This folder is empty", if self.can_write() { "Upload files here, or make a new folder." } else { "" })
            });
        } else {
            dom::set_html(&self.list, &rows.iter().enumerate().map(|(i, e)| self.row_html(i, e, trash)).collect::<String>());
        }
        let left = self.total.get().saturating_sub(rows.len());
        dom::set_html(&self.more, &if !trash && left > 0 { format!(r#"<button type="button" class="btn" id="fm-more-btn">Show more ({left})</button>"#) } else { String::new() });
        self.paint_sel();
    }

    fn paint_sel(&self) {
        {
            let sel = self.sel.borrow();
            for row in dom::qsa_in(&self.list, ".fm-row") {
                let Ok(i) = dom::data(&row, "i").parse::<usize>() else { continue };
                let on = sel.is(i);
                dom::toggle_class(&row, "sel", on);
                dom::toggle_class(&row, "cur", sel.cursor == Some(i) && sel.count() > 0);
                let _ = row.set_attribute("aria-selected", if on { "true" } else { "false" });
                if let Some(c) = dom::qs_in(&row, ".fm-check") { dom::set_checked(&c, on); }
            }
        }
        self.paint_tools();
    }

    fn paint_tools(&self) {
        let n = self.sel.borrow().count();
        let w = self.can_write();
        let btn = |tool: &str, ic: &str, label: &str, cls: &str| format!(r#"<button type="button" class="btn sm {cls}" data-tool="{tool}">{ic}{}</button>"#, esc(label));
        let reload = format!(r#"<button type="button" class="btn sm ghost icon" data-tool="reload" title="Reload" aria-label="Reload">{}</button>"#, icon("refresh"));
        let mut b: Vec<String> = vec![];
        if n > 0 {
            b.push(format!(r#"<span class="count">{n} selected</span>"#));
            if self.in_trash() {
                b.push(btn("restore", &svg_restore(), "Restore", ""));
                b.push(btn("forever", &icon("trash"), "Delete forever", "danger"));
            } else {
                let one = if n == 1 { self.picked().first().and_then(|i| self.row(*i)) } else { None };
                if w && one.is_some() { b.push(btn("rename", &icon("edit"), "Rename", "")); }
                if w && one.as_ref().is_some_and(logic::is_audio) { b.push(btn("tags", &icon("tag"), "Edit tags", "")); }
                if w {
                    b.push(btn("move", &svg_move(), "Move", ""));
                    b.push(btn("trash", &icon("trash"), "Move to trash", "danger"));
                }
            }
            if n < self.rows.borrow().len() { b.push(btn("all", "", "Select all", "ghost")); }
            b.push(btn("none", "", "Cancel", "ghost"));
        } else if self.in_trash() {
            let total = self.rows.borrow().len();
            if self.ready.get() && total > 0 {
                b.push(format!(r#"<span class="small muted">{total} in the trash</span>"#));
                b.push(btn("empty", &icon("trash"), "Empty trash", "danger"));
            }
            b.push(reload);
        } else {
            let (k, d) = self.sort.borrow().clone();
            b.push(btn("sort", &svg_sort(), &logic::sort_button(&k, &d), ""));
            if w {
                b.push(btn("new", &svg_folder_plus(), "New folder", ""));
                b.push(btn("upload", &svg_upload(), "Upload", ""));
            }
            if self.ready.get() && !self.via_link.get() { b.push(btn("import", &icon("download"), "Import…", "")); }
            b.push(reload);
        }
        dom::set_html(&self.tools, &b.concat());
    }

    // ---- the tree ----

    fn paint_roots(&self) {
        let html: String = self.roots.borrow().iter()
            .map(|r| node_html(&r.id, "", &r.name, 0, &if r.is_trash() { icon("trash") } else { svg_folder() }, &root_sub(r), r.is_trash(), &r.path, true)).collect();
        dom::set_html(&self.tree, &html);
    }

    fn find_node(&self, root: &str, path: &str) -> Option<Element> {
        dom::qsa_in(&self.tree, ".fm-node").into_iter().find(|n| dom::data(n, "root") == root && dom::data(n, "path") == path)
    }

    /// The open folder is marked in the tree.
    fn tree_mark(&self) {
        let (r, p) = self.cur();
        for n in dom::qsa_in(&self.tree, ".fm-node") {
            if let Some(row) = n.first_element_child() {
                let on = dom::data(&n, "root") == r && dom::data(&n, "path") == p;
                dom::toggle_class(&row, "on", on);
                if let Some(b) = dom::qs_in(&row, ".fm-nm") { if on { let _ = b.set_attribute("aria-current", "true"); } else { let _ = b.remove_attribute("aria-current"); } }
            }
        }
    }

    async fn tree_toggle(self: Rc<Self>, node: Element, want: Option<bool>) {
        let open = want.unwrap_or(!dom::has_class(&node, "open"));
        dom::toggle_class(&node, "open", open);
        if let Some(tw) = dom::qs_in(&node, ".fm-tw") { let _ = tw.set_attribute("aria-expanded", if open { "true" } else { "false" }); }
        if open && dom::data(&node, "loaded") != "1" { self.tree_load(node).await; }
    }

    /// A tree folder's subfolders (links to folders too: they open when their target is in a root).
    async fn tree_load(self: Rc<Self>, node: Element) {
        let (r, p) = (dom::data(&node, "root"), dom::data(&node, "path"));
        let depth: usize = dom::data(&node, "depth").parse().unwrap_or(0);
        let Some(kids) = node.last_element_child() else { return };
        dom::set_html(&kids, &format!(r#"<div class="fm-msg" style="padding-left:{}px">Loading…</div>"#, pad(depth + 1) + 22));
        let res = api::get(&format!("/files/list?root={}&path={}&sort=name&dir=asc&limit=500", enc(&r), enc(&p))).await;
        match res {
            Ok(d) => {
                let dirs: Vec<Value> = d.a("entries").into_iter().filter(logic::is_dir).collect();
                dom::set_data(&node, "loaded", "1");
                if let Some(tw) = dom::qs_in(&node, ".fm-tw") { dom::toggle_class(&tw, "leaf", dirs.is_empty()); }
                dom::set_html(&kids, &dirs.iter().map(|e| {
                    let name = e.s("name");
                    // a link to a folder: its own icon, and its target as the tooltip
                    let (ic, title) = if logic::is_link(e) { (icon("link"), logic::entry_line(e)) } else { (svg_folder(), String::new()) };
                    node_html(&r, &logic::join(&p, &name), &name, depth + 1, &ic, "", false, &title, false)
                }).collect::<String>());
            }
            Err(e) => dom::set_html(&kids, &format!(r#"<div class="fm-msg" style="padding-left:{}px">{}</div>"#, pad(depth + 1) + 22, esc(&e.message))),
        }
        self.tree_mark();
    }

    /// The folders above the open one are opened in the tree, and the open one is marked.
    async fn tree_reveal(self: Rc<Self>) {
        let (r, p) = self.cur();
        if r == "trash" { self.tree_mark(); return; }
        let parts = logic::crumbs(&p);
        let mut chain = vec![String::new()];
        chain.extend(parts.iter().take(parts.len().saturating_sub(1)).map(|(_, at)| at.clone()));
        for at in chain {
            if self.cur() != (r.clone(), p.clone()) { return; }
            let Some(node) = self.find_node(&r, &at) else { break };
            if !dom::has_class(&node, "open") || dom::data(&node, "loaded") != "1" { self.clone().tree_toggle(node, Some(true)).await; }
        }
        self.tree_mark();
    }

    /// A folder's subfolders changed (a new folder, a move, a rename): read again where the tree shows them.
    fn tree_refresh(self: &Rc<Self>, root: &str, path: &str) {
        let Some(node) = self.find_node(root, path) else { return };
        if dom::data(&node, "loaded") != "1" { return; }
        dom::set_data(&node, "loaded", "0");
        if dom::has_class(&node, "open") { dom::spawn(self.clone().tree_load(node)); }
        else if let Some(kids) = node.last_element_child() { dom::set_html(&kids, ""); }
    }

    /// The roots' free space again (after uploads and deletes).
    async fn refresh_free(self: Rc<Self>) {
        let Ok(d) = api::get("/files/roots").await else { return };
        let fresh: Vec<Root> = (if d.is_array() { d.items() } else { d.a("roots") }).iter().filter_map(Root::from).collect();
        for r in self.roots.borrow_mut().iter_mut() {
            if let Some(f) = fresh.iter().find(|f| f.id == r.id) { r.free = f.free; r.writable = f.writable; }
        }
        let roots = self.roots.borrow().clone();
        for r in &roots {
            if let Some(sub) = self.find_node(&r.id, "").and_then(|n| dom::qs_in(&n, ".fm-sub")) { dom::set_text(&sub, &root_sub_text(r)); }
        }
        self.paint_info();
    }

    // ---- what the rows do ----

    fn open(self: &Rc<Self>, i: usize) {
        if self.in_trash() { return; }
        let Some(e) = self.row(i) else { return };
        let (r, p) = self.cur();
        if logic::is_dir(&e) { self.go(&r, &logic::join(&p, &e.s("name"))); return; }
        let id = logic::track_id(&e);
        if id > 0 { play(id); }
    }

    /// The ⋯ menu of row `i` (a right-click opens it too). A row outside the selection becomes the selection; a
    /// row inside a selection of several gets the menu for all of them.
    fn row_menu(self: &Rc<Self>, anchor: &Element, i: usize) {
        let multi = { let s = self.sel.borrow(); s.is(i) && s.count() > 1 };
        if !multi { self.sel.borrow_mut().only(i); self.paint_sel(); }
        let idx = self.picked();
        let w = self.can_write();
        let mut items: Vec<Option<MenuItem>> = vec![];
        if self.in_trash() {
            { let (me, idx) = (self.clone(), idx.clone()); items.push(MenuItem::new("Restore", &svg_restore(), move || dom::spawn(me.clone().restore(idx.clone())))); }
            items.push(MenuItem::SEP);
            { let (me, idx) = (self.clone(), idx.clone()); items.push(MenuItem::danger("Delete forever", &icon("trash"), move || dom::spawn(me.clone().delete_forever(idx.clone())))); }
        } else if multi {
            if w {
                { let (me, idx) = (self.clone(), idx.clone()); items.push(MenuItem::new(&format!("Move {} items", idx.len()), &svg_move(), move || me.move_items(idx.clone()))); }
                { let (me, idx) = (self.clone(), idx.clone()); items.push(MenuItem::danger("Move to trash", &icon("trash"), move || dom::spawn(me.clone().trash_items(idx.clone())))); }
            }
        } else {
            let Some(e) = self.row(i) else { return };
            let id = logic::track_id(&e);
            if logic::is_dir(&e) { let me = self.clone(); items.push(MenuItem::new("Open", &svg_folder(), move || me.open(i))); }
            if id > 0 { items.push(MenuItem::new("Play", &icon("play"), move || play(id))); }
            if w && logic::is_audio(&e) { let me = self.clone(); items.push(MenuItem::new("Edit tags", &icon("tag"), move || me.edit_tags(i))); }
            if id > 0 { items.push(MenuItem::new("Edit song details…", &icon("edit"), move || fix_song(id))); }
            if logic::is_dir(&e) && !logic::is_link(&e) && !self.via_link.get() {
                let (me, folder) = (self.clone(), logic::join(&self.path.borrow(), &e.s("name")));
                items.push(MenuItem::new("Import…", &icon("download"), move || me.import(folder.clone())));
            }
            // on a link these act on the link itself, never on what it points to
            if w {
                if !items.is_empty() { items.push(MenuItem::SEP); }
                { let me = self.clone(); items.push(MenuItem::new("Rename", &icon("edit"), move || me.rename(i))); }
                { let me = self.clone(); items.push(MenuItem::new("Move", &svg_move(), move || me.move_items(vec![i]))); }
                { let me = self.clone(); items.push(MenuItem::danger("Move to trash", &icon("trash"), move || dom::spawn(me.clone().trash_items(vec![i])))); }
            }
        }
        if items.is_empty() { return; }
        ui::pop_menu(anchor, items);
    }

    fn new_folder(self: &Rc<Self>) {
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let (me, (root, path)) = (self.clone(), self.cur());
        dialogs::name_dialog("New folder", "", "Make folder", true, Rc::new(move |name: String| -> LocalBoxFuture<'static, Result<(), String>> {
            let (me, root, path) = (me.clone(), root.clone(), path.clone());
            Box::pin(async move {
                match api::post("/files/mkdir", json!({ "root": root, "path": path, "name": name })).await {
                    Ok(_) => {
                        toast("Folder made", "ok");
                        me.reload(vec![name]);
                        me.tree_refresh(&root, &path);
                        Ok(())
                    }
                    Err(e) => Err(e.message),
                }
            })
        }));
    }

    fn rename(self: &Rc<Self>, i: usize) {
        let Some(e) = self.row(i) else { return };
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let old = e.s("name");
        let (me, (root, path)) = (self.clone(), self.cur());
        let from = logic::join(&path, &old);
        dialogs::name_dialog("Rename", &old, "Rename", e.s("kind") == "dir", Rc::new(move |name: String| -> LocalBoxFuture<'static, Result<(), String>> {
            let (me, root, path, from) = (me.clone(), root.clone(), path.clone(), from.clone());
            Box::pin(async move {
                match api::post("/files/rename", json!({ "root": root, "path": from, "name": name })).await {
                    Ok(_) => {
                        toast("Renamed", "ok");
                        me.reload(vec![name]);
                        me.tree_refresh(&root, &path);
                        Ok(())
                    }
                    Err(e) => Err(e.message),
                }
            })
        }));
    }

    fn move_items(self: &Rc<Self>, idx: Vec<usize>) {
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let names: Vec<String> = idx.iter().filter_map(|i| self.row(*i)).map(|e| e.s("name")).collect();
        if names.is_empty() { return; }
        let title = if names.len() == 1 { format!("Move “{}”", names[0]) } else { format!("Move {} items", names.len()) };
        let (root, path) = self.cur();
        let from: Vec<String> = names.iter().map(|n| logic::join(&path, n)).collect();
        let me = self.clone();
        let (r2, p2, f2) = (root.clone(), path.clone(), from.clone());
        dialogs::folder_picker(self.roots.borrow().clone(), &root, &path, &title, (root.clone(), from), Rc::new(move |to_root: String, to_path: String| {
            if to_root == r2 && to_path == p2 { return; }
            let (me, root, path, from) = (me.clone(), r2.clone(), p2.clone(), f2.clone());
            dom::spawn(async move {
                match api::post("/files/move", json!({ "root": root, "from": from, "to_root": to_root, "to_path": to_path })).await {
                    Ok(a) => {
                        let failed = logic::failure_line(&a);
                        if failed.is_empty() { toast(&logic::done_line("move", a.i("moved").max(0) as usize), "ok"); } else { toast(&failed, "err"); }
                    }
                    Err(e) => toast_error(&e),
                }
                me.reload(vec![]);
                me.tree_refresh(&root, &path);
                me.tree_refresh(&to_root, &to_path);
            });
        }));
    }

    async fn trash_items(self: Rc<Self>, idx: Vec<usize>) {
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let names: Vec<String> = idx.iter().filter_map(|i| self.row(*i)).map(|e| e.s("name")).collect();
        if names.is_empty() { return; }
        let title = if names.len() == 1 { format!("Move “{}” to trash?", names[0]) } else { format!("Move {} items to trash?", names.len()) };
        if !confirm_dialog(&title, "You can restore them from Trash. Songs in the library are taken out of it until they are restored.", "Move to trash", true).await { return; }
        let (root, path) = self.cur();
        let paths: Vec<String> = names.iter().map(|n| logic::join(&path, n)).collect();
        match api::post("/files/trash", json!({ "root": root, "paths": paths })).await {
            Ok(a) => {
                let failed = logic::failure_line(&a);
                if failed.is_empty() { toast(&logic::done_line("trash", a.i("trashed").max(0) as usize), "ok"); } else { toast(&failed, "err"); }
            }
            Err(e) => toast_error(&e),
        }
        self.reload(vec![]);
        self.tree_refresh(&root, &path);
    }

    fn edit_tags(self: &Rc<Self>, i: usize) {
        let Some(e) = self.row(i) else { return };
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let (root, path) = self.cur();
        let name = e.s("name");
        let me = self.clone();
        tags::tags_modal(root, logic::join(&path, &name), Rc::new(move || me.reload(vec![name.clone()])));
    }

    fn import(self: &Rc<Self>, folder: String) {
        let Some(r) = self.root_info() else { return };
        let me = self.clone();
        import::import_modal(r, folder, Rc::new(move || me.reload_same()));
    }

    /// The files picked or dropped go to `dest` (a folder of the open root) once the upload dialog says how.
    fn ask_upload(self: &Rc<Self>, files: Vec<(web_sys::File, String)>, dest: String) {
        if !self.can_write() { toast(self.why_not(), "err"); return; }
        let root = self.root.borrow().clone();
        let place = self.place(&dest);
        upload::confirm(files, root, dest, place);
    }

    // ---- the trash ----

    async fn restore(self: Rc<Self>, idx: Vec<usize>) {
        let items: Vec<Value> = idx.iter().filter_map(|i| self.row(*i)).collect();
        if items.is_empty() { return; }
        let ids: Vec<String> = items.iter().map(|e| e.s("id")).collect();
        match api::post("/files/restore", json!({ "items": ids })).await {
            Ok(a) => {
                let failed = logic::failure_line(&a);
                if failed.is_empty() { toast(&logic::done_line("restore", a.i("restored").max(0) as usize), "ok"); } else { toast(&failed, "err"); }
            }
            Err(e) => toast_error(&e),
        }
        self.reload(vec![]);
        for e in &items { self.tree_refresh(&e.s("root"), &logic::parent_of(&e.s("path"))); }
    }

    async fn delete_forever(self: Rc<Self>, idx: Vec<usize>) {
        let items: Vec<Value> = idx.iter().filter_map(|i| self.row(*i)).collect();
        if items.is_empty() { return; }
        let name = |e: &Value| if e.s("name").is_empty() { logic::last_part(&e.s("path")).to_string() } else { e.s("name") };
        let title = if items.len() == 1 { format!("Delete “{}” forever?", name(&items[0])) } else { format!("Delete {} items forever?", items.len()) };
        if !confirm_dialog(&title, "This can't be undone.", "Delete forever", true).await { return; }
        let (mut done, mut first_err) = (0usize, String::new());
        for e in &items {
            match api::del(&format!("/files/trash/{}", enc(&e.s("id")))).await {
                Ok(_) => done += 1,
                Err(err) => { if first_err.is_empty() { first_err = format!("{}: {}", name(e), err.message); } }
            }
        }
        if first_err.is_empty() { toast(&logic::done_line("delete", done), "ok"); }
        else if items.len() - done > 1 { toast(&format!("{} weren't done. {first_err}", items.len() - done), "err"); }
        else { toast(&first_err, "err"); }
        self.reload(vec![]);
        dom::spawn(self.clone().refresh_free());
    }

    async fn empty_trash(self: Rc<Self>) {
        if !confirm_dialog("Empty the trash?", "Everything in it is deleted forever. This can't be undone.", "Empty trash", true).await { return; }
        match api::post_empty("/files/empty-trash").await {
            Ok(_) => toast("The trash is empty", "ok"),
            Err(e) => toast_error(&e),
        }
        self.reload(vec![]);
        dom::spawn(self.clone().refresh_free());
    }

    // ---- sorting ----

    fn set_sort(self: &Rc<Self>, key: &str) {
        let next = { let s = self.sort.borrow(); logic::next_sort((&s.0, &s.1), key) };
        dom::ls_set(SORT_KEY, &json!({ "sort": next.0, "dir": next.1 }).to_string());
        *self.sort.borrow_mut() = next;
        self.paint_head();
        self.paint_tools();
        self.reload_same();
    }

    fn sort_menu(self: &Rc<Self>, anchor: &Element) {
        let (k, d) = self.sort.borrow().clone();
        let items = logic::SORTS.iter().map(|(key, label)| {
            let me = self.clone();
            let text = if *key == k { format!("{label} ({})", logic::order_text(key, &d)) } else { label.to_string() };
            MenuItem::new(&text, &if *key == k { icon("check") } else { svg_blank() }, move || me.set_sort(key))
        }).collect();
        ui::pop_menu(anchor, items);
    }

    fn upload_menu(self: &Rc<Self>, anchor: &Element) {
        let view = self.view.clone();
        let v2 = view.clone();
        ui::pop_menu(anchor, vec![
            MenuItem::new("Upload files", &svg_upload(), move || { if let Some(i) = dom::qs_in(&view, "#fm-pick-files") { dom::click(&i); } }),
            MenuItem::new("Upload a folder", &svg_folder(), move || { if let Some(i) = dom::qs_in(&v2, "#fm-pick-dir") { dom::click(&i); } }),
        ]);
    }

    // ---- dragging files in ----

    /// The folder row under the pointer, when it is a plain folder (nothing is written through a link).
    fn drop_row(&self, e: &Event) -> Option<(Element, String)> {
        let row = dom::closest(e, ".fm-row")?;
        let i: usize = dom::data(&row, "i").parse().ok()?;
        let entry = self.row(i)?;
        (entry.s("kind") == "dir").then(|| (row, logic::join(&self.path.borrow(), &entry.s("name"))))
    }

    fn drop_clear(&self) {
        dom::remove_class(&self.wrap, "dropping");
        for r in dom::qsa_in(&self.list, ".fm-row.drop") { dom::remove_class(&r, "drop"); }
        self.hint_text.borrow_mut().clear();
    }

    // ---- wiring ----

    fn wire(self: &Rc<Self>) {
        // the tree: the arrow opens and closes a folder, the name opens it on the right
        {
            let me = self.clone();
            dom::on(&self.tree, "click", move |e| {
                let Some(node) = dom::closest(e, ".fm-node") else { return };
                if dom::closest(e, ".fm-tw").is_some() { dom::spawn(me.clone().tree_toggle(node, None)); return; }
                if dom::closest(e, ".fm-nm").is_none() { return; }
                let (r, p) = (dom::data(&node, "root"), dom::data(&node, "path"));
                if r != "trash" && !dom::has_class(&node, "open") { dom::spawn(me.clone().tree_toggle(node, Some(true))); }
                me.go(&r, &p);
            });
        }
        // the crumbs and the path box
        {
            let me = self.clone();
            dom::on(&self.crumbs, "click", move |e| {
                if dom::closest(e, "#fm-path-edit").is_some() { me.edit_path(); return; }
                if dom::closest(e, "#fm-path-cancel").is_some() { me.paint_crumbs(); return; }
                let Some(b) = dom::closest(e, "[data-crumb]") else { return };
                if dom::has_class(&b, "cur") { return; }
                let r = me.root.borrow().clone();
                me.go(&r, &dom::data(&b, "crumb"));
            });
        }
        {
            let me = self.clone();
            dom::on(&self.crumbs, "submit", move |e| {
                dom::prevent(e);
                let typed = logic::typed_path(&dom::value_of(&me.crumbs, "#fm-path"));
                let r = me.root.borrow().clone();
                me.go(&r, &typed);
            });
        }
        {
            let me = self.clone();
            dom::on(&self.crumbs, "keydown", move |e| {
                if dom::key(e) == "Escape" && dom::closest(e, "#fm-path").is_some() { dom::stop(e); me.paint_crumbs(); focus_still(&me.list); }
            });
        }
        // the toolbar
        {
            let me = self.clone();
            dom::on(&self.tools, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-tool]") else { return };
                let picked = me.picked();
                match dom::data(&b, "tool").as_str() {
                    "sort" => me.sort_menu(&b),
                    "new" => me.new_folder(),
                    "upload" => me.upload_menu(&b),
                    "import" => { let p = me.path.borrow().clone(); me.import(p); }
                    "reload" => { me.reload_same(); let r = me.root.borrow().clone(); let p = me.path.borrow().clone(); me.tree_refresh(&r, &p); dom::spawn(me.clone().refresh_free()); }
                    "rename" => { if let Some(i) = picked.first() { me.rename(*i); } }
                    "tags" => { if let Some(i) = picked.first() { me.edit_tags(*i); } }
                    "move" => me.move_items(picked),
                    "trash" => dom::spawn(me.clone().trash_items(picked)),
                    "restore" => dom::spawn(me.clone().restore(picked)),
                    "forever" => dom::spawn(me.clone().delete_forever(picked)),
                    "empty" => dom::spawn(me.clone().empty_trash()),
                    "all" => { me.sel.borrow_mut().all(); me.paint_sel(); }
                    "none" => { me.sel.borrow_mut().clear(); me.paint_sel(); }
                    _ => {}
                }
            });
        }
        {
            let me = self.clone();
            dom::on(&self.head, "click", move |e| { if let Some(b) = dom::closest(e, "[data-sort]") { me.set_sort(&dom::data(&b, "sort")); } });
        }
        {
            let me = self.clone();
            dom::on(&self.more, "click", move |e| { if dom::closest(e, "#fm-more-btn").is_some() { dom::spawn(me.clone().load_more()); } });
        }
        // the rows: a click picks (Ctrl / ⌘ adds, Shift a range), a double click opens; on a phone a tap opens a
        // folder and picks a file, as the phone app does
        {
            let me = self.clone();
            dom::on(&self.list, "click", move |e| {
                if dom::closest(e, "[data-top]").is_some() { let r = me.root.borrow().clone(); me.go(&r, ""); return; }
                let Some(row) = dom::closest(e, ".fm-row") else {
                    if dom::closest(e, "button, a, input").is_none() && me.sel.borrow().count() > 0 { me.sel.borrow_mut().clear(); me.paint_sel(); }
                    return;
                };
                let Ok(i) = dom::data(&row, "i").parse::<usize>() else { return };
                if let Some(b) = dom::closest(e, "[data-act=menu]") { me.row_menu(&b, i); return; }
                if dom::closest(e, ".fm-check").is_some() { me.sel.borrow_mut().toggle(i); me.paint_sel(); return; }
                let (ctrl, shift) = e.dyn_ref::<web_sys::MouseEvent>().map(|m| (m.ctrl_key() || m.meta_key(), m.shift_key())).unwrap_or((false, false));
                if dom::is_phone() && !ctrl && !shift {
                    let picking = me.sel.borrow().count() > 0;
                    if !picking && !me.in_trash() && me.row(i).is_some_and(|x| logic::is_dir(&x)) { me.open(i); return; }
                    me.sel.borrow_mut().toggle(i);
                    me.paint_sel();
                    return;
                }
                me.sel.borrow_mut().click(i, ctrl, shift);
                me.paint_sel();
            });
        }
        {
            let me = self.clone();
            dom::on(&self.list, "dblclick", move |e| {
                if dom::closest(e, "[data-act=menu], .fm-check").is_some() { return; }
                let Some(row) = dom::closest(e, ".fm-row") else { return };
                if let Ok(i) = dom::data(&row, "i").parse::<usize>() { me.open(i); }
            });
        }
        {
            let me = self.clone();
            dom::on(&self.list, "keydown", move |e| me.key(e));
        }
        {
            let me = self.clone();
            dom::on(&self.list, "focus", move |_| me.used.set(true));
        }
        // files dragged in from the computer: onto the list (this folder) or onto a folder row (that folder)
        {
            let me = self.clone();
            dom::on(&self.wrap, "dragover", move |e| {
                if !upload::has_files(e) { return; }
                dom::prevent(e);
                if !me.can_write() { upload::set_drop_effect(e, "none"); return; }
                upload::set_drop_effect(e, "copy");
                dom::add_class(&me.wrap, "dropping");
                let target = me.drop_row(e);
                for r in dom::qsa_in(&me.list, ".fm-row.drop") {
                    if target.as_ref().map(|(t, _)| t != &r).unwrap_or(true) { dom::remove_class(&r, "drop"); }
                }
                let dest = match &target { Some((row, p)) => { dom::add_class(row, "drop"); p.clone() } None => me.path.borrow().clone() };
                let text = format!("Drop to upload to {}", me.place(&dest));
                if *me.hint_text.borrow() != text { dom::set_text(&me.hint, &text); *me.hint_text.borrow_mut() = text; }
            });
        }
        {
            let me = self.clone();
            dom::on(&self.wrap, "dragleave", move |e| {
                let related = js_sys::Reflect::get(e.as_ref(), &"relatedTarget".into()).ok().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                if related.is_some_and(|n| me.wrap.contains(Some(&n))) { return; }
                me.drop_clear();
            });
        }
        {
            let me = self.clone();
            dom::on(&self.wrap, "drop", move |e| {
                if !upload::has_files(e) { return; }
                dom::prevent(e);
                let dest = me.drop_row(e).map(|(_, p)| p).unwrap_or_else(|| me.path.borrow().clone());
                me.drop_clear();
                if !me.can_write() { toast(me.why_not(), "err"); return; }
                // the drop's contents are taken now: the browser empties them when this handler returns
                let dropped = upload::take_drop(e);
                let me = me.clone();
                dom::spawn(async move {
                    let files = upload::read_drop(dropped).await;
                    me.ask_upload(files, dest);
                });
            });
        }
        // a file dropped beside the list must not open in the browser and leave the page
        dom::on(&self.view, "dragover", |e| { if upload::has_files(e) { dom::prevent(e); upload::set_drop_effect(e, "none"); } });
        dom::on(&self.view, "drop", |e| { if upload::has_files(e) { dom::prevent(e); } });
        // Upload files / Upload a folder
        for sel in ["#fm-pick-files", "#fm-pick-dir"] {
            let me = self.clone();
            dom::on_sel(&self.view, sel, "change", move |e| {
                let Some(input) = dom::target_el(e).and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
                let files = upload::from_input(&input);
                input.set_value("");
                let dest = me.path.borrow().clone();
                me.ask_upload(files, dest);
            });
        }
        // an upload batch ended: the folder open shows what arrived
        {
            let me = self.clone();
            dom::on(&dom::window(), upload::DONE_EVENT, move |_| {
                if !me.view.is_connected() { return; }
                if !me.in_trash() { me.reload_same(); }
                let (r, p) = me.cur();
                me.tree_refresh(&r, &p);
                dom::spawn(me.clone().refresh_free());
            });
        }
    }

    /// The list's keys.
    fn key(self: &Rc<Self>, e: &Event) {
        if overlay_open() { return; }
        let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
        let key = k.key();
        let (ctrl, shift, alt) = (k.ctrl_key() || k.meta_key(), k.shift_key(), k.alt_key());
        let n = self.rows.borrow().len() as i64;
        let trash = self.in_trash();
        match key.as_str() {
            "ArrowDown" | "ArrowUp" | "PageDown" | "PageUp" | "Home" | "End" => {
                dom::prevent(e);
                if n == 0 { return; }
                let delta = match key.as_str() { "ArrowDown" => 1, "ArrowUp" => -1, "PageDown" => 10, "PageUp" => -10, "Home" => -n, _ => n };
                self.sel.borrow_mut().step(delta, shift);
                self.paint_sel();
                let cur = self.sel.borrow().cursor;
                if let Some(row) = cur.and_then(|c| dom::qsa_in(&self.list, ".fm-row").into_iter().find(|r| dom::data(r, "i") == c.to_string())) { scroll_near(&row); }
            }
            "Enter" => {
                let s = self.sel.borrow().clone();
                let at = if s.count() == 1 { s.picked().first().copied() } else { s.cursor.filter(|c| s.is(*c)) };
                if let Some(i) = at { dom::prevent(e); self.open(i); }
            }
            "Backspace" if ctrl => { dom::prevent(e); self.delete_key(); }
            "Backspace" => { dom::prevent(e); if !trash { self.up(); } }
            "Delete" => { dom::prevent(e); self.delete_key(); }
            "F2" => {
                let p = self.picked();
                if p.len() == 1 && !trash { dom::prevent(e); self.rename(p[0]); }
            }
            "Escape" => { if self.sel.borrow().count() > 0 { self.sel.borrow_mut().clear(); self.paint_sel(); } }
            "a" | "A" if ctrl => { dom::prevent(e); self.sel.borrow_mut().all(); self.paint_sel(); }
            _ if key.chars().count() == 1 && !ctrl && !alt && key != " " => {
                let names: Vec<String> = if trash { self.rows.borrow().iter().map(|r| r.s("name")).collect() } else { self.keys() };
                let from = self.sel.borrow().cursor;
                if let Some(i) = logic::jump_to(&names, from, &key) {
                    self.sel.borrow_mut().only(i);
                    self.paint_sel();
                    if let Some(row) = dom::qsa_in(&self.list, ".fm-row").into_iter().find(|r| dom::data(r, "i") == i.to_string()) { scroll_near(&row); }
                }
            }
            _ => {}
        }
    }

    /// Delete: the picked rows go to trash (asked first); in the trash they are deleted forever (asked first).
    fn delete_key(self: &Rc<Self>) {
        let p = self.picked();
        if p.is_empty() { return; }
        if self.in_trash() { dom::spawn(self.clone().delete_forever(p)); }
        else if self.can_write() { dom::spawn(self.clone().trash_items(p)); }
        else { toast(self.why_not(), "err"); }
    }
}

/// The indent of a tree level.
fn pad(depth: usize) -> usize { 4 + depth * 14 }

/// "1.2 TB free · Read-only" under a root's name.
fn root_sub_text(r: &Root) -> String {
    let mut parts = vec![];
    if r.free > 0.0 { parts.push(format!("{} free", fmt::size(r.free))); }
    if !r.writable && !r.is_trash() { parts.push("Read-only".to_string()); }
    parts.join(" · ")
}
fn root_sub(r: &Root) -> String { format!(r#"<span class="fm-sub ellipsis">{}</span>"#, esc(&root_sub_text(r))) }

/// One folder of the tree: its arrow, its name (opens it), and the place its subfolders go.
#[allow(clippy::too_many_arguments)]
fn node_html(root: &str, path: &str, name: &str, depth: usize, ic: &str, sub: &str, leaf: bool, title: &str, is_root: bool) -> String {
    format!(r#"<div class="fm-node{rc}" data-root="{r}" data-path="{p}" data-depth="{depth}" role="treeitem">
      <div class="fm-node-row" style="padding-left:{pad}px">
        <button type="button" class="fm-tw{lf}" tabindex="-1" aria-label="Folders in {n}" aria-expanded="false">{chev}</button>
        <button type="button" class="fm-nm" title="{t}">{ic}<span class="grow" style="min-width:0"><span class="ellipsis" style="display:block">{n}</span>{sub}</span></button>
      </div>
      <div class="fm-kids" role="group"></div></div>"#,
        rc = if is_root { " root" } else { "" }, r = esc(root), p = esc(path), pad = pad(depth), lf = if leaf { " leaf" } else { "" },
        n = esc(name), chev = icon("chevron"), t = esc(if title.is_empty() { name } else { title }))
}

/// Plays a song the library has.
fn play(id: i64) {
    dom::spawn(async move {
        match api::get(&format!("/library/tracks/{id}")).await {
            Ok(t) if t.is_object() => crate::player::play_queue(vec![t], 0, "single"),
            _ => toast("That song isn't in the library", "err"),
        }
    });
}

/// The Fix song dialog (Library → a song's ⋯ → Edit song details…) for a song the library has.
fn fix_song(id: i64) {
    dom::spawn(async move {
        match api::get(&format!("/library/tracks/{id}")).await {
            Ok(t) if t.is_object() => crate::pages::library::fix_song_modal(&t),
            _ => toast("That song isn't in the library", "err"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_read_leniently() {
        let r = Root::from(&json!({"id": "library", "name": "Library", "path": "/music", "writable": true, "free_bytes": 1_000_000_000})).unwrap();
        assert!(r.writable && !r.is_trash() && r.free > 0.0);
        let t = Root::from(&json!({"id": "trash"})).unwrap();
        assert_eq!(t.name, "trash");
        assert!(t.is_trash() && t.writable);
        assert!(!Root::from(&json!({"id": "slskd", "writable": false})).unwrap().writable);
        assert!(Root::from(&json!({"name": "x"})).is_none());
        assert_eq!(root_sub_text(&Root { id: "staging".into(), name: "Staging".into(), path: String::new(), writable: false, free: 0.0 }), "Read-only");
    }

    #[test]
    fn a_tree_node_is_escaped_and_marked() {
        let h = node_html("library", "A \"b\"/C", "C", 2, &svg_folder(), "", false, "", false);
        assert!(h.contains(r#"data-path="A &quot;b&quot;/C""#), "{h}");
        assert!(h.contains(r#"data-depth="2""#) && h.contains("padding-left:32px"), "{h}");
        assert!(!h.contains("fm-node root") && !h.contains("fm-tw leaf"));
        assert!(node_html("trash", "", "Trash", 0, &icon("trash"), "", true, "", true).contains("fm-tw leaf"));
    }
}
