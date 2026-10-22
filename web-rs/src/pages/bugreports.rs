//! Bug reports (sidebar, under the storage box): any user writes a report – Bug / Not a bug / How do I… – with
//! images or text files; the admin reads them in the app, sets a status, replies in a thread the reporter sees
//! (and gets by email when that is set up) and exports them as a zip for the developer.
//! Server side: server/src/routes/bugreports.rs (/api/bug-reports).
use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{confirm_dialog, esc, fmt, icon, modal, toast, ModalOpts};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, File, FileList, FormData, HtmlInputElement};

/// The three kinds, as the server stores them and as the user reads them.
/// Every kind: the list's chips and the labels; `crash` is a crash log the phone app sends (the Crash logs section).
pub const KINDS: [(&str, &str); 4] = [("bug", "Bug"), ("not_bug", "Not a bug"), ("howto", "How do I…"), ("crash", "Crash log")];
/// The kinds a new report can pick (a crash log comes from the app).
const PICK_KINDS: &[(&str, &str)] = &[("bug", "Bug"), ("not_bug", "Not a bug"), ("howto", "How do I…")];
/// The five statuses: key, label, badge class.
pub const STATUSES: [(&str, &str, &str); 5] = [
    ("new", "New", "warn"),
    ("looking", "Looking into it", "info"),
    ("fixed", "Fixed", "ok"),
    ("wontfix", "Won't fix", ""),
    ("answered", "Answered", "accent"),
];
const MAX_FILES: usize = 6;
const MAX_BYTES: f64 = 8.0 * 1024.0 * 1024.0;
const FILTER_KEY: &str = "encore.bugFilter";

fn kind_label(k: &str) -> &'static str { KINDS.iter().find(|(key, _)| *key == k).map(|(_, l)| *l).unwrap_or("Bug") }
fn kind_badge(k: &str) -> String {
    let cls = match k { "bug" => "err", "howto" => "info", "crash" => "warn", _ => "" };
    format!(r#"<span class="badge {cls} bug-kind" data-kind="{}">{}</span>"#, esc(k), kind_label(k))
}
fn status_label(s: &str) -> &'static str { STATUSES.iter().find(|(key, _, _)| *key == s).map(|(_, l, _)| *l).unwrap_or("New") }
fn status_badge(s: &str) -> String {
    let cls = STATUSES.iter().find(|(key, _, _)| *key == s).map(|(_, _, c)| *c).unwrap_or("");
    format!(r#"<span class="badge {cls} bug-status" data-status="{}">{}</span>"#, esc(s), status_label(s))
}
fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// The server's version as the app knows it (/api/auth/info), "" when it has none.
fn app_version() -> String { app::info().s("version") }
/// `navigator.userAgent` (at most 200 chars) + the screen size.
fn device() -> String {
    let ua = dom::window().navigator().user_agent().unwrap_or_default();
    let ua: String = ua.chars().take(200).collect();
    let screen = dom::window().screen().ok().and_then(|s| Some(format!("{}x{}", s.width().ok()?, s.height().ok()?))).unwrap_or_default();
    if screen.is_empty() { ua } else { format!("{ua} · {screen}") }
}

/// `fetch('/api' + path, { method: 'POST', body: formData })`: the answer as JSON, or the server's sentence.
async fn post_form(path: &str, fd: FormData) -> Result<Value, String> {
    let req = gloo_net::http::RequestBuilder::new(&format!("/api{path}"))
        .method(gloo_net::http::Method::POST)
        .header("X-Requested-With", "encore")
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .body(fd)
        .map_err(|_| "Bad request".to_string())?;
    let r = req.send().await.map_err(|_| "Not connected to the server".to_string())?;
    let text = r.text().await.unwrap_or_default();
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !r.ok() {
        if r.status() == 401 { dom::dispatch("encore:unauthorized", None); }
        let detail = data.g("detail");
        return Err(if truthy(detail) { js_string(detail) } else if r.status_text().is_empty() { "Request failed".to_string() } else { r.status_text() });
    }
    Ok(data)
}

fn changed() { dom::dispatch("encore:bugs-changed", None); }

// ---- the file picker behind an "Add images" button (the New report modal and the reply box) ------------
struct Picker {
    files: RefCell<Vec<File>>,
    urls: RefCell<Vec<String>>,
    thumbs: Element,
}
impl Picker {
    /// Wires `btn` (opens the hidden `input`) and draws the picked files as thumbnails into `thumbs`.
    fn bind(root: &Element, btn: &str, input: &str, thumbs: &str) -> Option<Rc<Picker>> {
        let (btn, input, thumbs) = (dom::qs_in(root, btn)?, dom::qs_in(root, input)?, dom::qs_in(root, thumbs)?);
        let p = Rc::new(Picker { files: RefCell::new(vec![]), urls: RefCell::new(vec![]), thumbs });
        {
            let input = input.clone();
            dom::on(&btn, "click", move |e| { e.prevent_default(); dom::click(&input); });
        }
        {
            let p = p.clone();
            dom::on(&input, "change", move |e| {
                let Some(i) = dom::target_el(e).and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else { return };
                if let Some(list) = i.files() { p.add(list); }
                i.set_value("");
            });
        }
        {
            let p2 = p.clone();
            dom::on(&p.thumbs, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-rm]") else { return };
                e.prevent_default();
                let i: usize = dom::data(&b, "rm").parse().unwrap_or(usize::MAX);
                { let mut f = p2.files.borrow_mut(); if i < f.len() { f.remove(i); } }
                p2.draw();
            });
        }
        Some(p)
    }
    fn add(&self, list: FileList) {
        for i in 0..list.length() {
            let Some(f) = list.get(i) else { continue };
            if self.files.borrow().len() >= MAX_FILES { toast(&format!("At most {MAX_FILES} files per report"), "err"); break; }
            if f.size() > MAX_BYTES { toast(&format!("{} is over 8 MB", f.name()), "err"); continue; }
            self.files.borrow_mut().push(f);
        }
        self.draw();
    }
    fn draw(&self) {
        for u in self.urls.borrow_mut().drain(..) { let _ = web_sys::Url::revoke_object_url(&u); }
        let files = self.files.borrow();
        let html: String = files.iter().enumerate().map(|(i, f)| {
            let name = f.name();
            let img = f.type_().starts_with("image/");
            let url = if img { web_sys::Url::create_object_url_with_blob(f).ok() } else { None };
            if let Some(u) = &url { self.urls.borrow_mut().push(u.clone()); }
            let inner = match url {
                Some(u) => format!(r#"<img src="{}" alt="{}">"#, esc(&u), esc(&name)),
                None => format!(r#"<span class="small ellipsis" title="{0}">{0}</span>"#, esc(&name)),
            };
            format!(r#"<div class="bug-thumb" data-file="{i}" title="{}">{inner}<button type="button" class="bug-thumb-x" data-rm="{i}" aria-label="Remove {}">×</button></div>"#, esc(&name), esc(&name))
        }).collect();
        dom::set_html(&self.thumbs, &html);
    }
    fn files(&self) -> Vec<File> { self.files.borrow().clone() }
    fn append_to(&self, fd: &FormData) {
        for f in self.files.borrow().iter() { let _ = fd.append_with_blob_and_filename("file", f, &f.name()); }
    }
}

/// The thumbnails / links of a report's or a message's files.
fn files_html(files: &[Value]) -> String {
    if files.is_empty() { return String::new(); }
    let items: String = files.iter().map(|f| {
        let (url, name, mime) = (f.s("url"), f.s("name"), f.s("mime"));
        if mime.starts_with("image/") {
            format!(r#"<a class="bug-file img" target="_blank" rel="noopener" href="{0}" title="{1}"><img src="{0}" alt="{1}" loading="lazy"></a>"#, esc(&url), esc(&name))
        } else {
            format!(r#"<a class="bug-file text" target="_blank" rel="noopener" href="{}">{}<span>{}</span><span class="faint">{}</span></a>"#, esc(&url), icon("external"), esc(&name), fmt::size(f.f("size")))
        }
    }).collect();
    format!(r#"<div class="bug-files">{items}</div>"#)
}

// ---- the list filter (kept in localStorage) ---------------------------------------------------------
#[derive(Clone, Default)]
struct Filter { status: String, kind: String, q: String, mine: bool }
impl Filter {
    fn load(admin: bool) -> Filter {
        let v = dom::ls_get(FILTER_KEY).and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
        let status = { let s = v.s("status"); if STATUSES.iter().any(|(k, _, _)| *k == s) { s } else { "all".into() } };
        let kind = { let k = v.s("kind"); if KINDS.iter().any(|(key, _)| *key == k) { k } else { String::new() } };
        Filter { status, kind, q: v.s("q"), mine: admin && v.truthy("mine") }
    }
    fn save(&self) { dom::ls_set(FILTER_KEY, &json!({ "status": self.status, "kind": self.kind, "q": self.q, "mine": self.mine }).to_string()); }
    /// `status=&kind=` for the list and the export link (only what is set).
    fn params(&self) -> Vec<String> {
        let mut p = vec![];
        if self.status != "all" && !self.status.is_empty() { p.push(format!("status={}", enc(&self.status))); }
        if !self.kind.is_empty() { p.push(format!("kind={}", enc(&self.kind))); }
        p
    }
    fn active(&self) -> bool { (self.status != "all" && !self.status.is_empty()) || !self.kind.is_empty() || !self.q.trim().is_empty() }
}

fn export_href(f: &Filter) -> String {
    let mut p = f.params();
    // a plain text file (the zip with the pictures: without format=txt)
    p.push("format=txt".into());
    format!("/api/bug-reports/export?{}", p.join("&"))
}

fn report_row(r: &Value) -> String {
    let id = r.i("id");
    let n = r.i("message_count");
    let count = match n { 0 => "No replies".to_string(), 1 => "1 reply".to_string(), n => format!("{n} replies") };
    let files = r.i("file_count");
    format!(r##"<div class="list-item bug-row{unread_cls}" data-id="{id}" style="flex-wrap:wrap;gap:10px">
    <span class="dot warn bug-unread{hidden}" title="New since you last looked"></span>
    {kind}
    <div class="grow" style="min-width:200px"><a class="bug-title" href="#/bug-reports/{id}" style="font-weight:650">{title}</a>
      <div class="small faint">by {user} · {when}</div></div>
    {status}
    <span class="small muted bug-count">{count}{files}</span>
  </div>"##,
        unread_cls = if r.truthy("unread") { " unread" } else { "" },
        hidden = if r.truthy("unread") { "" } else { " hidden" },
        kind = kind_badge(&r.s("kind")),
        title = esc(&r.s("title")),
        user = esc(&r.s("username")),
        when = fmt::ago(if r.f("updated_at") > 0.0 { r.f("updated_at") } else { r.f("created_at") }),
        status = status_badge(&r.s("status")),
        files = if files > 0 { format!(" · {files} file{}", if files == 1 { "" } else { "s" }) } else { String::new() })
}

async fn load_list(list: &Element, f: &Filter, admin: bool) -> Result<(), String> {
    let mut p = f.params();
    if !f.q.trim().is_empty() { p.push(format!("q={}", enc(f.q.trim()))); }
    if admin && f.mine { p.push("mine=1".into()); }
    let d = api::get(&format!("/bug-reports{}", if p.is_empty() { String::new() } else { format!("?{}", p.join("&")) })).await.map_err(|e| e.message)?;
    let rows = d.a("reports");
    let html = if rows.is_empty() {
        if f.active() {
            format!(r#"<div class="empty" id="bugs-empty">{}<p class="muted">Nothing matches these filters.</p></div>"#, icon("bug"))
        } else {
            format!(r#"<div class="empty" id="bugs-empty">{}<p class="muted">No reports yet. New report writes the first one.</p></div>"#, icon("bug"))
        }
    } else {
        format!(r#"<div class="list">{}</div>"#, rows.iter().map(report_row).collect::<String>())
    };
    dom::set_html(list, &html);
    Ok(())
}

/// #/bug-reports – the list.
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = app::is_admin_user(&ctx.user);
        let filter = Rc::new(RefCell::new(Filter::load(admin)));
        let (status_tabs, kind_chips, scope, export) = {
            let f = filter.borrow();
            // the tab row reads "Looking" (the badge and the export keep the long label): six tabs have to fit a phone
            let tabs: String = std::iter::once(("all", "All")).chain(STATUSES.iter().map(|(k, l, _)| (*k, if *k == "looking" { "Looking" } else { *l })))
                .map(|(k, l)| format!(r#"<button data-s="{k}" class="{}">{l}</button>"#, if k == f.status { "active" } else { "" })).collect();
            let chips: String = KINDS.iter().map(|(k, l)| format!(r#"<button class="badge{} bug-chip" data-k="{k}">{l}</button>"#, if *k == f.kind { " accent on" } else { "" })).collect();
            let scope = if admin {
                format!(r#"<div class="seg" id="bugs-scope"><button data-v="all" class="{}">Everyone</button><button data-v="mine" class="{}">Mine</button></div>"#,
                    if f.mine { "" } else { "on" }, if f.mine { "on" } else { "" })
            } else { String::new() };
            let export = if admin { format!(r#"<a class="btn sm" id="bugs-export" href="{}" download>{}Export</a>"#, esc(&export_href(&f)), icon("download")) } else { String::new() };
            (tabs, chips, scope, export)
        };
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px;gap:10px">
      <h1 style="margin:0">Bug reports</h1>
      <div class="row wrap" style="gap:8px">
        {scope}
        {export}
        <button class="btn primary" id="bugs-new">{plus}New report</button>
      </div>
    </div>
    <div class="tabs" id="bugs-status">{status_tabs}</div>
    <div class="row wrap mt" style="gap:10px">
      <div class="chips" id="bugs-kind">{kind_chips}</div>
      <input class="input" id="bugs-q" type="search" placeholder="Search reports" value="{q}" style="max-width:260px;padding:7px 12px">
    </div>
    <div class="card flat mt" id="bugs-list"><div class="empty"><span class="spinner"></span></div></div>"#,
            plus = icon("plus"), q = esc(&filter.borrow().q)));
        let list = dom::qs_in(&view, "#bugs-list").ok_or("#bugs-list")?;
        let reload: Rc<dyn Fn()> = {
            let (list, filter, view) = (list.clone(), filter.clone(), view.clone());
            Rc::new(move || {
                let f = filter.borrow().clone();
                f.save();
                if let Some(a) = dom::qs_in(&view, "#bugs-export") { let _ = a.set_attribute("href", &export_href(&f)); }
                let list = list.clone();
                dom::spawn(async move { if let Err(e) = load_list(&list, &f, admin).await { dom::set_html(&list, &format!(r#"<div class="error-box">{}</div>"#, esc(&e))); } });
            })
        };
        {
            let (v, filter, reload) = (view.clone(), filter.clone(), reload.clone());
            dom::on_sel(&view, "#bugs-status", "click", move |e| {
                let Some(t) = dom::closest(e, "[data-s]") else { return };
                filter.borrow_mut().status = dom::data(&t, "s");
                for b in dom::qsa_in(&v, "#bugs-status button") { dom::toggle_class(&b, "active", b == t); }
                reload();
            });
        }
        {
            let (v, filter, reload) = (view.clone(), filter.clone(), reload.clone());
            dom::on_sel(&view, "#bugs-kind", "click", move |e| {
                let Some(t) = dom::closest(e, "[data-k]") else { return };
                let k = dom::data(&t, "k");
                let next = if filter.borrow().kind == k { String::new() } else { k };
                filter.borrow_mut().kind = next.clone();
                for b in dom::qsa_in(&v, "#bugs-kind button") {
                    let on = !next.is_empty() && dom::data(&b, "k") == next;
                    dom::toggle_class(&b, "accent", on);
                    dom::toggle_class(&b, "on", on);
                }
                reload();
            });
        }
        {
            let (filter, reload) = (filter.clone(), reload.clone());
            let deb = Rc::new(crate::ui::Debounce::new(250, move || reload()));
            dom::on_sel(&view, "#bugs-q", "input", move |e| {
                filter.borrow_mut().q = dom::target_el(e).map(|t| dom::value(&t)).unwrap_or_default();
                deb.run();
            });
        }
        if admin {
            let (v, filter, reload) = (view.clone(), filter.clone(), reload.clone());
            dom::on_sel(&view, "#bugs-scope", "click", move |e| {
                let Some(t) = dom::closest(e, "[data-v]") else { return };
                filter.borrow_mut().mine = dom::data(&t, "v") == "mine";
                for b in dom::qsa_in(&v, "#bugs-scope button") { dom::toggle_class(&b, "on", b == t); }
                reload();
            });
        }
        dom::on_sel(&view, "#bugs-new", "click", |_| new_report_modal());
        // a click anywhere on a row opens it (the title is a real link for middle-click and the keyboard)
        dom::on(&list, "click", |e| {
            if dom::closest(e, "a").is_some() { return; }
            let Some(row) = dom::closest(e, "[data-id]") else { return };
            app::navigate(&format!("/bug-reports/{}", dom::data(&row, "id")));
        });
        let f = filter.borrow().clone();
        load_list(&list, &f, admin).await?;
        // the badge agrees with the rows it stands next to (a reply may have come in since the last refresh)
        changed();
        {
            let reload = reload.clone();
            dom::interval(&view, 60_000, move || reload());
        }
        Ok(())
    })
}

/// The New report modal: kind, title, description, images; sends FormData to POST /api/bug-reports.
pub fn new_report_modal() {
    let page = app::previous_hash();
    let version = app_version();
    let seg: String = PICK_KINDS.iter().enumerate().map(|(i, (k, l))| format!(r#"<button type="button" data-v="{k}" class="{}">{l}</button>"#, if i == 0 { "on" } else { "" })).collect();
    let body = format!(r#"<div class="stack" style="gap:12px">
      <div class="seg" id="br-kind">{seg}</div>
      <label class="field"><span>Title</span><input class="input" id="br-title" maxlength="200" placeholder="A short name for it"></label>
      <label class="field"><span>Description</span><textarea class="input" id="br-body" rows="6" maxlength="20000" placeholder="What happened, what you expected, how to repeat it"></textarea></label>
      <div class="row wrap" style="gap:10px">
        <button type="button" class="btn sm" id="br-add">{upload}Add images</button>
        <input type="file" id="br-files" multiple accept="image/*,.txt,.log" hidden>
        <span class="small muted">Up to {max} files, 8 MB each. Images, .txt and .log files.</span>
      </div>
      <div class="bug-thumbs" id="br-thumbs"></div>
      <div class="small muted" id="br-meta">Sent along: {page}the app version ({version}) and your browser.</div>
    </div>"#,
        upload = icon("plus"), max = MAX_FILES,
        page = if page.is_empty() { String::new() } else { format!("the page you came from ({}), ", esc(&page)) },
        version = esc(if version.is_empty() { "unknown" } else { &version }));
    let m = modal(ModalOpts::new("New report", &body).footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="br-send">Send</button>"#));
    let kind = Rc::new(RefCell::new("bug".to_string()));
    {
        let (m2, kind) = (m.clone(), kind.clone());
        if let Some(sg) = m.qs("#br-kind") {
            dom::on(&sg, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-v]") else { return };
                *kind.borrow_mut() = dom::data(&b, "v");
                for x in m2.qsa("#br-kind button") { dom::toggle_class(&x, "on", x == b); }
            });
        }
    }
    let Some(picker) = Picker::bind(&m.el, "#br-add", "#br-files", "#br-thumbs") else { return };
    let Some(send) = m.qs("#br-send") else { return };
    if let Some(t) = m.qs("#br-title") { dom::focus(&t); }
    let send2 = send.clone();
    dom::on(&send, "click", move |_| {
        let title = m.qs("#br-title").map(|e| dom::value(&e)).unwrap_or_default().trim().to_string();
        let text = m.qs("#br-body").map(|e| dom::value(&e)).unwrap_or_default().trim().to_string();
        if title.is_empty() { toast("Give the report a title", "err"); if let Some(t) = m.qs("#br-title") { dom::focus(&t); } return; }
        if text.is_empty() { toast("Say what happened", "err"); if let Some(t) = m.qs("#br-body") { dom::focus(&t); } return; }
        let Ok(fd) = FormData::new() else { return };
        let _ = fd.append_with_str("kind", &kind.borrow());
        let _ = fd.append_with_str("title", &title);
        let _ = fd.append_with_str("body", &text);
        let _ = fd.append_with_str("page", &page);
        let _ = fd.append_with_str("app_version", &version);
        let _ = fd.append_with_str("device", &device());
        picker.append_to(&fd);
        dom::set_disabled(&send2, true);
        send2.set_text_content(Some("Sending…"));
        let (m, send) = (m.clone(), send2.clone());
        dom::spawn(async move {
            match post_form("/bug-reports", fd).await {
                Ok(r) => {
                    m.close();
                    toast("Report sent. Thanks.", "ok");
                    changed();
                    app::navigate(&format!("/bug-reports/{}", r.i("id")));
                }
                Err(e) => { toast(&e, "err"); dom::set_disabled(&send, false); send.set_text_content(Some("Send")); }
            }
        });
    });
}

fn message_html(m: &Value) -> String {
    format!(r#"<div class="list-item bug-msg" data-mid="{id}" style="align-items:flex-start;flex-wrap:wrap;gap:10px">
    <div class="grow" style="min-width:0">
      <div class="row wrap" style="gap:8px"><b>{user}</b>{admin}<span class="small faint">{when}</span></div>
      <div class="bug-msg-body" style="white-space:pre-wrap;margin-top:4px">{body}</div>
      {files}
    </div>
  </div>"#,
        id = m.i("id"), user = esc(&m.s("username")),
        admin = if m.truthy("is_admin") { r#"<span class="badge accent">Admin</span>"# } else { "" },
        when = fmt::date(m.f("created_at")), body = esc(&m.s("body")), files = files_html(&m.a("files")))
}

fn rerender(view: Element, ctx: Ctx) {
    dom::spawn(async move { if let Err(e) = render_one(view.clone(), ctx).await { if view.is_connected() { dom::set_html(&view, &format!(r#"<div class="error-box">{}</div>"#, esc(&e))); } } });
}

/// #/bug-reports/<id> – one report with its thread and the reply box.
pub fn render_one(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let id = ctx.param(0);
        let admin = app::is_admin_user(&ctx.user);
        if dom::qs_in(&view, "#bug-title").is_none() {
            dom::set_html(&view, r##"<a href="#/bug-reports" class="small muted" id="bug-back">← Bug reports</a><div class="skeleton mt" style="height:200px"></div>"##);
        }
        let d = api::get(&format!("/bug-reports/{id}")).await.map_err(|e| e.message)?;
        // the server marked it seen: the sidebar badge may drop
        changed();
        let r = d.g("report");
        let msgs = d.a("messages");
        let status = r.s("status");
        let status_html = if admin {
            format!(r#"<select class="input" id="bug-status" style="width:auto;padding:5px 10px" aria-label="Status">{}</select>"#,
                STATUSES.iter().map(|(k, l, _)| format!(r#"<option value="{k}"{}>{l}</option>"#, if *k == status { " selected" } else { "" })).collect::<String>())
        } else { status_badge(&status) };
        let page = r.s("page");
        let page_html = if page.is_empty() { String::new() } else if page.starts_with("#/") { format!(r#" · page <a href="{0}">{0}</a>"#, esc(&page)) } else { format!(" · page {}", esc(&page)) };
        let admin_tools = if admin {
            format!(r#"<a class="btn sm" id="bug-export" href="/api/bug-reports/export?ids={id}&format=txt" download>{}Export this report</a><button class="btn sm danger" id="bug-delete">{}Delete</button>"#, icon("download"), icon("trash"))
        } else { String::new() };
        let thread = if msgs.is_empty() { r#"<div class="empty" style="padding:24px"><p class="muted small" style="margin:0">No replies yet.</p></div>"#.to_string() }
            else { format!(r#"<div class="list">{}</div>"#, msgs.iter().map(message_html).collect::<String>()) };
        dom::set_html(&view, &format!(r##"
    <a href="#/bug-reports" class="small muted" id="bug-back">← Bug reports</a>
    <div class="row wrap between" style="gap:10px;margin-top:10px">
      <div class="row wrap" style="gap:10px;min-width:0">{kind}<h1 id="bug-title" style="margin:0;overflow-wrap:anywhere">{title}</h1>{status}</div>
      <div class="row wrap" style="gap:8px">{admin_tools}</div>
    </div>
    <div class="small muted" id="bug-meta" style="margin-top:8px;overflow-wrap:anywhere">by {user} · {when}{page}{version}{device}</div>
    <div class="card mt" id="bug-report">
      <div id="bug-body" style="white-space:pre-wrap;overflow-wrap:anywhere">{body}</div>
      {files}
    </div>
    <div class="section-head"><h2>Replies</h2></div>
    <div class="card flat" id="bug-thread">{thread}</div>
    <div class="card mt stack" id="bug-reply" style="gap:10px">
      <textarea class="input" id="bug-reply-text" rows="3" maxlength="20000" placeholder="Write a reply"></textarea>
      <div class="row wrap" style="gap:8px">
        <button type="button" class="btn sm" id="bug-reply-add">{plus}Add images</button>
        <input type="file" id="bug-reply-files" multiple accept="image/*,.txt,.log" hidden>
        <div class="bug-thumbs grow" id="bug-reply-thumbs"></div>
        <button class="btn primary" id="bug-reply-send">Send</button>
      </div>
    </div>"##,
            kind = kind_badge(&r.s("kind")), title = esc(&r.s("title")), status = status_html,
            user = esc(&r.s("username")), when = fmt::date(r.f("created_at")), page = page_html,
            version = if r.truthy("app_version") { format!(" · app {}", esc(&r.s("app_version"))) } else { String::new() },
            device = if r.truthy("device") { format!(" · {}", esc(&r.s("device"))) } else { String::new() },
            body = esc(&r.s("body")), files = files_html(&r.a("files")), thread = thread, plus = icon("plus")));
        if admin {
            {
                let id = id.clone();
                dom::on_sel(&view, "#bug-status", "change", move |e| {
                    let Some(sel) = dom::target_el(e) else { return };
                    let (id, s) = (id.clone(), dom::value(&sel));
                    dom::spawn(async move {
                        match api::post(&format!("/bug-reports/{id}/status"), json!({ "status": s })).await {
                            Ok(_) => { toast(&format!("Status: {}", status_label(&s)), "ok"); changed(); }
                            Err(e) => toast(&e.message, "err"),
                        }
                    });
                });
            }
            {
                let id = id.clone();
                dom::on_sel(&view, "#bug-delete", "click", move |_| {
                    let id = id.clone();
                    dom::spawn(async move {
                        if !confirm_dialog("Delete this report?", "The report, its replies and its files are removed for good.", "Delete", true).await { return; }
                        match api::del(&format!("/bug-reports/{id}")).await {
                            Ok(_) => { toast("Report deleted", "ok"); changed(); app::navigate("/bug-reports"); }
                            Err(e) => toast(&e.message, "err"),
                        }
                    });
                });
            }
        }
        let Some(picker) = Picker::bind(&view, "#bug-reply-add", "#bug-reply-files", "#bug-reply-thumbs") else { return Ok(()) };
        if let Some(send) = dom::qs_in(&view, "#bug-reply-send") {
            let (v, c, send2, id) = (view.clone(), ctx.clone(), send.clone(), id.clone());
            dom::on(&send, "click", move |_| {
                let text = dom::value_of(&v, "#bug-reply-text").trim().to_string();
                let files = picker.files();
                if text.is_empty() && files.is_empty() { toast("Write a reply or add a file", "err"); return; }
                let Ok(fd) = FormData::new() else { return };
                let _ = fd.append_with_str("body", &text);
                picker.append_to(&fd);
                dom::set_disabled(&send2, true);
                let (v, c, send, id) = (v.clone(), c.clone(), send2.clone(), id.clone());
                dom::spawn(async move {
                    match post_form(&format!("/bug-reports/{id}/messages"), fd).await {
                        Ok(_) => { toast("Reply sent", "ok"); changed(); rerender(v, c); }
                        Err(e) => { toast(&e, "err"); dom::set_disabled(&send, false); }
                    }
                });
            });
        }
        Ok(())
    })
}
