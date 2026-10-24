//! Uploads (§4.3 of the client shapes): files and folders picked with the browser's file chooser or dropped on
//! the page, sent one file per request so each has its own answer and its progress shows. Folders keep their
//! parts in the file's name ("Album/CD1/01 Song.flac"), read from `webkitRelativePath` for a picked folder and
//! from `DataTransferItem.webkitGetAsEntry()` for a dropped one. The uploads go on while other pages are open:
//! the panel with "Uploading 3 of 12" and Stop sits on the page itself, and a second batch joins the first.

use super::logic;
use crate::dom;
use crate::json::J;
use crate::ui::{self, esc, fmt, toast, ModalOpts};
use js_sys::{Array, Function, Promise, Reflect};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Element, File, HtmlInputElement};

/// The event the page listens for: a batch of uploads ended (the open folder reloads).
pub const DONE_EVENT: &str = "encore:files-uploaded";

/// One file to send: the folder it goes to (root and path) and its name there.
pub struct Item {
    pub root: String,
    pub path: String,
    pub name: String,
    pub file: File,
    pub conflict: String,
}

fn get(o: &JsValue, k: &str) -> JsValue { Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED) }

/// `o[m](...args)`.
fn call(o: &JsValue, m: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let f: Function = get(o, m).dyn_into()?;
    f.apply(o, &args.iter().cloned().collect::<Array>())
}

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

// ---- what was picked or dropped ---------------------------------------------------------------------------------

/// The files of a file input: (file, its name relative to the upload's folder). A folder input names each file
/// with its folder ("Album/CD1/01 Song.flac", `webkitRelativePath`).
pub fn from_input(input: &HtmlInputElement) -> Vec<(File, String)> {
    let Some(list) = input.files() else { return vec![] };
    let mut out = vec![];
    for i in 0..list.length() {
        let Some(f) = list.get(i) else { continue };
        let rel = get(&f, "webkitRelativePath").as_string().filter(|s| !s.is_empty()).unwrap_or_else(|| f.name());
        // a folder chooser also hands over the hidden files of the folder (.DS_Store and the like)
        if rel.split('/').skip(1).any(|p| p.starts_with('.')) { continue; }
        out.push((f, rel));
    }
    out
}

/// What a drop held, taken while the drop event runs (the browser empties `dataTransfer` after it).
pub struct Dropped {
    entries: Vec<JsValue>,
    files: Vec<File>,
}

/// Whether a drag carries files from the computer (not text or a link from the page).
pub fn has_files(e: &web_sys::Event) -> bool {
    let types = get(&get(e.as_ref(), "dataTransfer"), "types");
    if types.is_undefined() || types.is_null() { return false; }
    Array::from(&types).iter().any(|t| t.as_string().as_deref() == Some("Files"))
}

/// Sets what the pointer shows over the drop place ("copy", or "none" where nothing may be written).
pub fn set_drop_effect(e: &web_sys::Event, effect: &str) {
    let dt = get(e.as_ref(), "dataTransfer");
    if dt.is_object() { let _ = Reflect::set(&dt, &"dropEffect".into(), &effect.into()); }
}

/// The dropped files and folders (`webkitGetAsEntry`), or the plain file list where a browser has no entries.
pub fn take_drop(e: &web_sys::Event) -> Dropped {
    let dt = get(e.as_ref(), "dataTransfer");
    let (mut entries, mut files) = (vec![], vec![]);
    let items = get(&dt, "items");
    let n = get(&items, "length").as_f64().unwrap_or(0.0) as u32;
    for i in 0..n {
        let it = Reflect::get_u32(&items, i).unwrap_or(JsValue::UNDEFINED);
        if get(&it, "kind").as_string().as_deref() != Some("file") { continue; }
        match call(&it, "webkitGetAsEntry", &[]).ok().filter(|x| x.is_object()) {
            Some(entry) => entries.push(entry),
            None => { if let Ok(f) = call(&it, "getAsFile", &[]).and_then(|f| f.dyn_into::<File>()) { files.push(f); } }
        }
    }
    if entries.is_empty() && files.is_empty() {
        let list = get(&dt, "files");
        let n = get(&list, "length").as_f64().unwrap_or(0.0) as u32;
        files = (0..n).filter_map(|i| Reflect::get_u32(&list, i).ok()?.dyn_into::<File>().ok()).collect();
    }
    Dropped { entries, files }
}

/// A callback API (`entry.file(ok, fail)`, `reader.readEntries(ok, fail)`) as a future.
async fn callback(o: &JsValue, method: &str) -> Option<JsValue> {
    let o2 = o.clone();
    let method = method.to_string();
    let p = Promise::new(&mut |ok, fail| {
        let fail2 = fail.clone();
        if call(&o2, &method, &[ok.into(), fail.into()]).is_err() { let _ = fail2.call0(&JsValue::NULL); }
    });
    JsFuture::from(p).await.ok()
}

/// Every file of a drop with its name relative to the drop ("Album/CD1/01 Song.flac"); the hidden files inside
/// dropped folders are left out. Sorted by name, so a folder goes up in order.
pub async fn read_drop(d: Dropped) -> Vec<(File, String)> {
    let mut out: Vec<(File, String)> = d.files.into_iter().map(|f| { let n = f.name(); (f, n) }).collect();
    let mut stack: Vec<(JsValue, String)> = d.entries.into_iter().map(|e| (e, String::new())).collect();
    while let Some((e, prefix)) = stack.pop() {
        let name = get(&e, "name").as_string().unwrap_or_default();
        if get(&e, "isDirectory").is_truthy() {
            let Ok(reader) = call(&e, "createReader", &[]) else { continue };
            let dir = format!("{prefix}{name}/");
            // readEntries hands over a folder a hundred entries at a time, then an empty list
            loop {
                let Some(batch) = callback(&reader, "readEntries").await else { break };
                let batch = Array::from(&batch);
                if batch.length() == 0 { break; }
                for c in batch.iter() {
                    if get(&c, "name").as_string().is_some_and(|n| n.starts_with('.')) { continue; }
                    stack.push((c, dir.clone()));
                }
            }
        } else if get(&e, "isFile").is_truthy() {
            if let Some(f) = callback(&e, "file").await.and_then(|f| f.dyn_into::<File>().ok()) { out.push((f, format!("{prefix}{name}"))); }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

// ---- the dialog before an upload ---------------------------------------------------------------------------------

/// "Upload “x”?" / "Upload 12 files?": where they go, and what happens to a name that is already there. Upload
/// queues them.
pub fn confirm(files: Vec<(File, String)>, root: String, path: String, place: String) {
    if files.is_empty() {
        toast("That folder has no files", "info");
        return;
    }
    let size: f64 = files.iter().map(|(f, _)| f.size()).sum();
    let title = if files.len() == 1 { format!("Upload “{}”?", logic::last_part(&files[0].1)) } else { format!("Upload {} files?", files.len()) };
    let opts = [("rename", "Keep both (the new one gets “(2)”)"), ("replace", "Replace it (the file that is there goes to Trash)"), ("skip", "Skip the new one")];
    let body = format!(r#"<p class="small muted" style="margin-top:0">To {}{}</p>
      <div style="font-weight:650;margin-top:14px">When a file with the same name is there</div>
      <div id="up-conflict">{}</div>"#,
        esc(&place), if size > 0.0 { format!(" · {}", fmt::size(size)) } else { String::new() },
        opts.iter().map(|(k, l)| format!(r#"<label class="fm-radio"><input type="radio" name="up-conflict" value="{k}"{}><span>{}</span></label>"#,
            if *k == "rename" { " checked" } else { "" }, esc(l))).collect::<String>());
    let m = ui::modal(ModalOpts::new(&esc(&title), &body)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="up-go">Upload</button>"#));
    let files = Rc::new(RefCell::new(Some(files)));
    let m2 = m.clone();
    dom::on_sel(&m.el, "#up-go", "click", move |_| {
        let conflict = m2.qsa("input[name=up-conflict]").into_iter().find(dom::checked).map(|i| dom::value(&i)).unwrap_or_else(|| "rename".into());
        let Some(files) = files.borrow_mut().take() else { return };
        m2.close();
        let items = files.into_iter().filter_map(|(file, rel)| {
            let name = logic::upload_name(&rel);
            (!name.is_empty()).then(|| Item { root: root.clone(), path: path.clone(), name, file, conflict: conflict.clone() })
        }).collect();
        start(items);
    });
}

// ---- the queue and its panel ------------------------------------------------------------------------------------

/// The uploads of this page load: what is left, what is done, and the panel.
struct Up {
    queue: RefCell<VecDeque<Item>>,
    count: Cell<usize>,
    done: Cell<usize>,
    ok: Cell<usize>,
    first_error: RefCell<String>,
    /// bytes of every file of the batch, of the finished ones, and of the one being sent so far
    total: Cell<f64>,
    sent: Cell<f64>,
    now: Cell<f64>,
    current: RefCell<String>,
    /// the request being sent (Stop aborts it)
    xhr: RefCell<Option<JsValue>>,
    stopped: Cell<bool>,
    panel: Element,
    _stop: dom::EventListener,
    /// leaving or reloading the page while files are going up asks first
    _unload: dom::EventListener,
}

thread_local! {
    static UP: RefCell<Option<Rc<Up>>> = const { RefCell::new(None) };
}

/// Whether files are going up from this page right now (a reload would cut them off; update.rs waits).
pub fn uploading() -> bool { UP.with(|u| u.borrow().is_some()) }

/// Queues `items`: a running batch takes them on, otherwise a new one starts.
pub fn start(items: Vec<Item>) {
    if items.is_empty() { return; }
    let bytes: f64 = items.iter().map(|i| i.file.size()).sum();
    if let Some(up) = UP.with(|u| u.borrow().clone()) {
        up.count.set(up.count.get() + items.len());
        up.total.set(up.total.get() + bytes);
        up.queue.borrow_mut().extend(items);
        paint(&up);
        return;
    }
    let panel = dom::create("div");
    panel.set_class_name("fm-up");
    let _ = panel.set_attribute("role", "status");
    dom::set_html(&panel, r#"<div class="row" style="gap:12px">
        <div class="grow"><div style="font-weight:650" data-up-n></div><div class="small muted ellipsis" data-up-name></div></div>
        <button class="btn sm" data-up-stop>Stop</button></div>
      <div class="progress"><i data-up-bar style="width:0%"></i></div>
      <div class="small faint" data-up-bytes></div>"#);
    let _ = dom::body().append_child(&panel);
    let stop_btn = dom::qs_in(&panel, "[data-up-stop]").unwrap_or_else(|| panel.clone());
    let stop = dom::listener(&stop_btn, "click", |_| stop());
    let unload = dom::listener(&dom::window(), "beforeunload", |e| {
        e.prevent_default();
        let _ = Reflect::set(e.as_ref(), &"returnValue".into(), &"".into());
    });
    let up = Rc::new(Up {
        count: Cell::new(items.len()), done: Cell::new(0), ok: Cell::new(0), first_error: RefCell::new(String::new()),
        total: Cell::new(bytes), sent: Cell::new(0.0), now: Cell::new(0.0), current: RefCell::new(String::new()),
        queue: RefCell::new(items.into()), xhr: RefCell::new(None), stopped: Cell::new(false), panel, _stop: stop, _unload: unload,
    });
    UP.with(|u| *u.borrow_mut() = Some(up.clone()));
    paint(&up);
    dom::spawn(run(up));
}

/// Stop: the file being sent is cut off (the server keeps nothing of it) and the rest are not sent.
fn stop() {
    let Some(up) = UP.with(|u| u.borrow().clone()) else { return };
    up.stopped.set(true);
    let xhr = up.xhr.borrow().clone();
    if let Some(x) = xhr { let _ = call(&x, "abort", &[]); }
}

fn paint(up: &Up) {
    let p = &up.panel;
    let n = (up.done.get() + 1).min(up.count.get());
    if let Some(el) = dom::qs_in(p, "[data-up-n]") { dom::set_text(&el, &format!("Uploading {n} of {}", up.count.get())); }
    if let Some(el) = dom::qs_in(p, "[data-up-name]") { dom::set_text(&el, &up.current.borrow()); }
    let total = up.total.get();
    let sent = up.sent.get() + up.now.get();
    let pct = if total > 0.0 { (sent / total * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    if let Some(el) = dom::qs_in(p, "[data-up-bar]") { dom::set_style(&el, "width", &format!("{pct:.1}%")); }
    if let Some(el) = dom::qs_in(p, "[data-up-bytes]") { dom::set_text(&el, &format!("{} of {}", fmt::size(sent.min(total)), fmt::size(total))); }
}

/// What one request came to.
enum Sent {
    Answer(Value),
    Failed(String),
    Stopped,
}

async fn run(up: Rc<Up>) {
    loop {
        if up.stopped.get() { break; }
        let Some(item) = up.queue.borrow_mut().pop_front() else { break };
        *up.current.borrow_mut() = item.name.clone();
        up.now.set(0.0);
        paint(&up);
        let size = item.file.size();
        let why = match send(&up, &item).await {
            Sent::Stopped => break,
            Sent::Answer(a) => logic::upload_outcome(&a).err(),
            Sent::Failed(msg) => Some(msg),
        };
        match why {
            None => up.ok.set(up.ok.get() + 1),
            Some(reason) => {
                let mut first = up.first_error.borrow_mut();
                if first.is_empty() { *first = format!("{}: {reason}", logic::last_part(&item.name)); }
            }
        }
        up.done.set(up.done.get() + 1);
        up.sent.set(up.sent.get() + size);
        up.now.set(0.0);
        paint(&up);
    }
    UP.with(|u| u.borrow_mut().take());
    up.panel.remove();
    let (msg, kind) = logic::upload_end(up.ok.get(), up.count.get(), up.stopped.get(), &up.first_error.borrow());
    toast(&msg, kind);
    dom::dispatch(DONE_EVENT, None);
}

/// Sends one file as a multipart `file` part, its filename the name in the folder (XMLHttpRequest, for the
/// upload's progress, which fetch doesn't give).
async fn send(up: &Rc<Up>, item: &Item) -> Sent {
    let url = format!("/api/files/upload?root={}&path={}&conflict={}", enc(&item.root), enc(&item.path), enc(&item.conflict));
    let Ok(ctor) = get(&dom::window(), "XMLHttpRequest").dyn_into::<Function>() else { return Sent::Failed("This browser can't upload files".into()) };
    let Ok(xhr) = Reflect::construct(&ctor, &Array::new()) else { return Sent::Failed("This browser can't upload files".into()) };
    if call(&xhr, "open", &["POST".into(), url.into()]).is_err() { return Sent::Failed("Request failed".into()); }
    let _ = call(&xhr, "setRequestHeader", &["X-Requested-With".into(), "encore".into()]);
    let _ = call(&xhr, "setRequestHeader", &["Accept".into(), "application/json".into()]);
    let Ok(form) = web_sys::FormData::new() else { return Sent::Failed("Request failed".into()) };
    if form.append_with_blob_and_filename("file", &item.file, &item.name).is_err() { return Sent::Failed("Request failed".into()); }
    let size = item.file.size();
    let up2 = up.clone();
    let progress = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
        if let Some(n) = get(&ev, "loaded").as_f64() {
            // the request is a little bigger than the file (the form's own lines)
            up2.now.set(n.min(size));
            paint(&up2);
        }
    });
    let _ = Reflect::set(&get(&xhr, "upload"), &"onprogress".into(), progress.as_ref());
    let x2 = xhr.clone();
    let ended = Promise::new(&mut |ok, _fail| { let _ = Reflect::set(&x2, &"onloadend".into(), &ok); });
    *up.xhr.borrow_mut() = Some(xhr.clone());
    if call(&xhr, "send", &[form.into()]).is_err() {
        up.xhr.borrow_mut().take();
        return Sent::Failed("Request failed".into());
    }
    let _ = JsFuture::from(ended).await;
    up.xhr.borrow_mut().take();
    drop(progress);
    if up.stopped.get() { return Sent::Stopped; }
    let status = get(&xhr, "status").as_f64().unwrap_or(0.0) as u16;
    let text = get(&xhr, "responseText").as_string().unwrap_or_default();
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status == 0 {
        dom::dispatch("encore:server-down", None);
        return Sent::Failed("Not connected to the server".into());
    }
    if (200..300).contains(&status) {
        return if data.is_object() { Sent::Answer(data) } else { Sent::Failed("Unexpected reply from the server".into()) };
    }
    if status == 401 { dom::dispatch("encore:unauthorized", None); }
    let detail = data.s("detail");
    Sent::Failed(if !detail.is_empty() { detail } else {
        let st = get(&xhr, "statusText").as_string().unwrap_or_default();
        if st.is_empty() { "Request failed".into() } else { st }
    })
}
