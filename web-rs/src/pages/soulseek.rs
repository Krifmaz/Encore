//! Port of web/js/pages/soulseek.js – see docs/ui-rewrite/CONTRACT.md
//! Soulseek: search the network, browse what a user shares, download files or whole folders, watch transfers.
//! Everything goes through /api/soulseek; a download becomes an ordinary request the worker tags and imports.

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, EventListener};
use crate::json::{js_string, json, J};
use crate::ui::{self, empty_state, esc, icon, toast, ModalOpts};
use gloo_events::EventListenerOptions;
use gloo_timers::callback::Timeout;
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use web_sys::Element;

const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];
const MAX_USERS: usize = 150;     // result cards rendered per search – the network can answer with far more
const MAX_FOLDERS: usize = 400;   // folders listed in the browse modal before asking for a filter

/// slskd transfer states → badge class + short label (anything else is prettified by `state_badge`); the JS `STATES`.
fn states(state: &str) -> Option<(&'static str, &'static str)> {
    Some(match state {
        "Requested" => ("", "Requested"),
        "Queued, Remotely" => ("info", "In their queue"),
        "Queued, Locally" => ("info", "Queued"),
        "Initializing" => ("accent", "Starting"),
        "InProgress" => ("accent", "Downloading"),
        "Completed, Succeeded" => ("ok", "Done"),
        "Completed, Errored" => ("err", "Failed"),
        "Completed, Cancelled" => ("", "Cancelled"),
        "Completed, TimedOut" => ("err", "Timed out"),
        "Completed, Rejected" => ("err", "Rejected"),
        "Completed, Aborted" => ("err", "Aborted"),
        _ => return None,
    })
}

/// The JS `Set` of "user\nfilename" keys of every ticked file.
type Checked = Rc<RefCell<HashSet<String>>>;
/// The JS `root._users`: the data the data-u / data-f indexes point into.
type Users = Rc<RefCell<Vec<Value>>>;

// ---- small helpers -------------------------------------------------------------------------------

/// `obj[key]` as an array slice (`[]` when it isn't one) – without cloning.
fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] { v.g(key).as_array().map(|a| a.as_slice()).unwrap_or(&[]) }
/// `encodeURIComponent(s)`.
fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }
/// JavaScript's `String.prototype.trim`.
fn js_trim(s: &str) -> String { String::from(js_sys::JsString::from(s).trim()) }
/// `root.querySelector(sel)` – the JS would throw on a missing element, we return the router's error.
fn need(root: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(root, sel).ok_or_else(|| format!("{sel} not found")) }

/// Last component of a Soulseek path (backslashes from Windows clients, slashes from others).
fn base_name(p: &str) -> String { p.split(['\\', '/']).filter(|s| !s.is_empty()).next_back().unwrap_or("").to_string() }
/// `String(f.extension || (f.name || baseName(f.filename)).split('.').pop() || '').replace(/^\./, '').toLowerCase()`.
fn ext_of(f: &Value) -> String {
    let e = if f.truthy("extension") { f.s("extension") } else {
        let name = if f.truthy("name") { f.s("name") } else { base_name(&f.s("filename")) };
        name.rsplit('.').next().unwrap_or("").to_string()
    };
    e.strip_prefix('.').unwrap_or(&e).to_lowercase()
}
fn file_key(u: &Value, f: &Value) -> String { format!("{}\n{}", u.s("username"), f.s("filename")) }
fn is_finished(t: &Value) -> bool { t.s("state").starts_with("Completed") }
fn is_active(t: &Value) -> bool { !is_finished(t) }
fn plural(n: f64, w: &str) -> String { format!("{} {}{}", ui::fmt::num(n), w, if n == 1.0 { "" } else { "s" }) }
fn speed_text(b: f64) -> String { if b >= 1e6 { format!("{:.1} MB/s", b / 1e6) } else { format!("{} kB/s", (b / 1e3).round() as i64) } }
/// `Math.max(0, Math.min(100, Math.round(t.progress ?? (t.size ? (t.bytes || 0) / t.size * 100 : 0))))`.
fn pct(t: &Value) -> i64 {
    let p = if !t.is_nullish("progress") { t.f("progress") } else if t.truthy("size") { t.f("bytes") / t.f("size") * 100.0 } else { 0.0 };
    p.round().clamp(0.0, 100.0) as i64
}
/// Free slot first, then the fastest uploaders.
fn sort_users(mut users: Vec<Value>) -> Vec<Value> {
    let slot = |u: &Value| if u.truthy("hasFreeUploadSlot") { 1 } else { 0 };
    users.sort_by(|a, b| slot(b).cmp(&slot(a)).then_with(|| b.f("uploadSpeed").partial_cmp(&a.f("uploadSpeed")).unwrap_or(std::cmp::Ordering::Equal)));
    users
}

/// Same text as the picker's quality badge: "FLAC 24-bit/96kHz", "MP3 320k".
fn quality(f: &Value) -> String {
    let ext = ext_of(f);
    let up = if ext.is_empty() { "?".to_string() } else { ext.to_uppercase() };
    if LOSSLESS.contains(&ext.as_str()) {
        let bits = if f.truthy("bitDepth") { format!(" {}-bit", f.s("bitDepth")) } else { String::new() };
        let khz = if f.truthy("sampleRate") && f.g("sampleRate").as_f64() != Some(44100.0) { format!("/{}kHz", js_string(&json!(f.f("sampleRate") / 1000.0))) } else { String::new() };
        return format!("{up}{bits}{khz}");
    }
    if f.truthy("bitRate") { format!("{up} {}k", f.s("bitRate")) } else { up }
}

/// `.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/,\s*/g, ' · ')`.
fn prettify(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut o = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ',' {
            o.push_str(" · ");
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() { i += 1; }
            continue;
        }
        o.push(c);
        if c.is_ascii_lowercase() && chars.get(i + 1).map(|n| n.is_ascii_uppercase()).unwrap_or(false) { o.push(' '); }
        i += 1;
    }
    o
}

/// Humanised slskd state as a badge; uploads say "Uploading" instead of "Downloading".
fn state_badge(state: &str, upload: bool) -> String {
    let (cls, mut label) = match states(state) {
        Some((c, l)) => (c, l.to_string()),
        None => ("", prettify(if state.is_empty() { "Unknown" } else { state })),
    };
    if upload && state == "InProgress" { label = "Uploading".into(); }
    let spin = if state == "InProgress" || state == "Initializing" { r#"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"# } else { "" };
    format!(r#"<span class="badge {cls}">{spin}{}</span>"#, esc(&label))
}

/// ui.toast() takes plain text; this one can carry a link.
fn toast_html(html: &str, kind: &str) {
    let el = dom::create("div");
    el.set_class_name(&format!("toast {kind}"));
    dom::set_html(&el, html);
    if let Some(t) = dom::by_id("toasts") { let _ = t.append_child(&el); }
    let e1 = el.clone();
    let e2 = el.clone();
    dom::keep_forever(Timeout::new(5000, move || { dom::set_style(&e1, "opacity", "0"); dom::set_style(&e1, "transition", "opacity .3s"); }));
    dom::keep_forever(Timeout::new(5400, move || e2.remove()));
}

/// "Signed in as X · sharing N files · port P" – or why it isn't working, linking to the settings.
fn status_html(s: &Value, error: &str) -> String {
    let settings = r##"<a href="#/settings?s=slskd" style="color:var(--accent)">Settings → Soulseek</a>"##;
    if !error.is_empty() { return format!(r#"<span class="dot err"></span> {} · {settings}"#, esc(error)); }
    let state = s.s("state");
    if s.truthy("configured") && state == "not_installed" {
        return format!(r#"<span class="dot err"></span> <span style="color:var(--warn)">Soulseek is not installed{}</span> · {settings}"#,
            if s.truthy("hint") { format!(" – {}", esc(&s.s("hint"))) } else { String::new() });
    }
    if !s.truthy("enabled") { return format!(r#"<span class="dot"></span> Soulseek is not enabled – turn it on in {settings}"#); }
    if state == "waiting_vpn" {
        return format!(r#"<span class="dot"></span> <span style="color:var(--warn)">{}</span> · {settings}"#,
            esc(&if s.truthy("error") { s.s("error") } else { "Waiting for the VPN".to_string() }));
    }
    if state == "crashed" {
        return format!(r#"<span class="dot err"></span> <span style="color:var(--warn)">Soulseek stopped{}</span> · {settings}"#,
            if s.truthy("error") { format!(" – {}", esc(&s.s("error"))) } else { String::new() });
    }
    if state == "starting" {
        return format!(r#"<span class="dot"></span> Soulseek is starting…{}"#,
            if s.truthy("error") { format!(r#" <span style="color:var(--warn)">{}</span>"#, esc(&s.s("error"))) } else { String::new() });
    }
    if !s.truthy("logged_in") {
        return format!(r#"<span class="dot err"></span> <span style="color:var(--warn)">Not signed in{}</span> · {settings}"#,
            if s.truthy("error") { format!(" – {}", esc(&s.s("error"))) } else { String::new() });
    }
    let mut parts = vec![format!(r#"<span class="dot ok"></span> Signed in as <b>{}</b>"#,
        esc(&if s.truthy("username") { s.s("username") } else { "?".to_string() }))];
    parts.push(if s.g("shares").truthy("files") { format!("sharing {}", plural(s.g("shares").f("files"), "file")) } else { "sharing nothing".to_string() });
    if !s["scanning"].is_null() { parts.push(format!("scanning shares {}%", esc(&s.s("scanning")))); }
    if s.truthy("port") { parts.push(format!("port {}", esc(&s.s("port")))); }
    if s.truthy("error") { parts.push(format!(r#"<span style="color:var(--warn)">{}</span>"#, esc(&s.s("error")))); }
    parts.join(" · ")
}

// ---- downloading -----------------------------------------------------------------------------------

/// POST /download: the worker fetches, tags and imports the files exactly like a picker pick.
async fn download(u: &Value, folder: &Value, files: &[Value], btn: &Element) -> bool {
    if files.is_empty() { return false; }
    dom::set_disabled(btn, true);
    // JSON.stringify drops undefined: only keys the objects actually have go out
    let mut body = Map::new();
    if u.has("username") { body.insert("username".into(), u.g("username").clone()); }
    if folder.has("directory") { body.insert("directory".into(), folder.g("directory").clone()); }
    body.insert("files".into(), Value::Array(files.iter().map(|f| {
        let mut m = Map::new();
        if f.has("filename") { m.insert("filename".into(), f.g("filename").clone()); }
        if f.has("size") { m.insert("size".into(), f.g("size").clone()); }
        Value::Object(m)
    }).collect()));
    body.insert("kind".into(), json!(if files.len() > 1 { "album" } else { "track" }));
    let ok = match api::post("/soulseek/download", Value::Object(body)).await {
        Ok(_) => {
            toast_html(r##"Added to your requests · <a href="#/requests" style="color:var(--accent);font-weight:600">View requests</a>"##, "ok");
            dom::dispatch("encore:requests-changed", None);
            true
        }
        Err(e) => { toast(&e.message, "err"); false }
    };
    dom::set_disabled(btn, false);
    ok
}

// ---- result markup (shared by search results and the browse modal) -----------------------------------

fn user_badges(u: &Value) -> String {
    (if u.truthy("hasFreeUploadSlot") { r#"<span class="badge ok">Free slot</span>"#.to_string() } else { format!(r#"<span class="badge warn">{} queued</span>"#, ui::fmt::num(u.f("queueLength"))) })
        + &(if u.truthy("uploadSpeed") { format!(r#" <span class="badge">{}</span>"#, esc(&speed_text(u.f("uploadSpeed")))) } else { String::new() })
}

fn file_row(f: &Value, k: usize, checked: bool) -> String {
    format!(r#"<div class="slsk-file">
    <label><input type="checkbox" data-i="{k}" {}><span class="name">{}</span></label>
    <span class="meta"><span class="badge {}">{}</span>{}<span>{}</span></span>
  </div>"#,
        if checked { "checked" } else { "" },
        esc(&if f.truthy("name") { f.s("name") } else { base_name(&f.s("filename")) }),
        if LOSSLESS.contains(&ext_of(f).as_str()) { "ok" } else { "" },
        esc(&quality(f)),
        if f.truthy("length") { format!(r#"<span class="len">{}</span>"#, ui::fmt::dur(f.f("length"))) } else { String::new() },
        ui::fmt::size(f.f("size")))
}

fn file_rows(u: &Value, f: &Value, checked: &HashSet<String>) -> String {
    arr(f, "files").iter().enumerate().map(|(k, x)| file_row(x, k, checked.contains(&file_key(u, x)))).collect()
}

fn selected_btn(u: &Value, f: &Value, checked: &HashSet<String>) -> String {
    let n = arr(f, "files").iter().filter(|x| checked.contains(&file_key(u, x))).count();
    format!(r#"<button class="btn sm" data-dl="selected" {}>{}Download selected{}</button>"#,
        if n > 0 { "" } else { "disabled" }, icon("check"), if n > 0 { format!(" ({n})") } else { String::new() })
}

/// One folder: name, full path, download buttons, file rows. Collapsible in the browse modal, open in results.
fn folder_html(u: &Value, i: usize, f: &Value, j: usize, checked: &HashSet<String>, collapsible: bool, open: bool) -> String {
    let size: f64 = arr(f, "files").iter().map(|x| x.f("size")).sum();
    let directory = f.s("directory");
    let bn = base_name(&directory);
    let head = format!(r#"<div class="slsk-folder-head">
      {}
      <div class="grow"><div class="name">{} <span class="faint small">· {} · {}</span></div>
        <div class="path">{}</div></div>
      <div class="row" style="gap:6px;flex:none">{}<button class="btn sm primary" data-dl="folder">{}Download folder</button></div>
    </div>"#,
        if collapsible { format!(r#"<span class="chev">{}</span>"#, icon("chevron")) } else { String::new() },
        esc(if bn.is_empty() { &directory } else { &bn }), plural(arr(f, "files").len() as f64, "file"), ui::fmt::size(size),
        esc(&directory),
        selected_btn(u, f, checked), icon("download"));
    let files = format!(r#"<div class="slsk-files" {}>{}</div>"#, if open { "" } else { "data-lazy" }, if open { file_rows(u, f, checked) } else { String::new() });
    if collapsible {
        format!(r#"<details class="slsk-folder" data-u="{i}" data-f="{j}" {}><summary>{head}</summary>{files}</details>"#, if open { "open" } else { "" })
    } else {
        format!(r#"<div class="slsk-folder" data-u="{i}" data-f="{j}">{head}{files}</div>"#)
    }
}

/// One search result: a collapsible card per user; its folders are rendered when it is first opened.
fn user_card(u: &Value, i: usize, checked: &HashSet<String>, open: bool) -> String {
    let folders = arr(u, "folders");
    let files: usize = folders.iter().map(|f| arr(f, "files").len()).sum();
    let username = u.s("username");
    format!(r#"<details class="card flat slsk-user" data-u="{i}" data-user="{un}" {}>
    <summary>
      <span class="chev">{}</span>
      <b class="ellipsis" style="max-width:240px">{un}</b>
      {}
      <span class="faint small">{} · {}</span>
      <span class="grow"></span>
      <button class="btn sm" data-browse="{un}">{}Browse</button>
    </summary>
    <div class="slsk-body" {}>{}</div>
  </details>"#,
        if open { "open" } else { "" },
        icon("chevron"),
        user_badges(u),
        plural(folders.len() as f64, "folder"), plural(files as f64, "file"),
        icon("user"),
        if open { "" } else { "data-lazy" },
        if open { folders.iter().enumerate().map(|(j, f)| folder_html(u, i, f, j, checked, false, true)).collect::<String>() } else { String::new() },
        un = esc(&username))
}

/// The JS `lookup(el)`: the folder box an element sits in and the data-u / data-f indexes.
fn lookup(el: &Element) -> Option<(Element, usize, usize)> {
    let bx = dom::closest_of(el, "[data-f]")?;
    let ui = dom::data(&bx, "u").parse().ok()?;
    let fi = dom::data(&bx, "f").parse().ok()?;
    Some((bx, ui, fi))
}

/// The JS `syncSel`: re-render the "Download selected" button of a folder.
fn sync_sel(bx: &Element, u: &Value, f: &Value, checked: &HashSet<String>) {
    if let Some(b) = dom::qs_in(bx, "[data-dl=selected]") { b.set_outer_html(&selected_btn(u, f, checked)); }
}

/// Wires a results container: lazy rendering of collapsed cards/folders, the checkboxes, the download
/// and Browse buttons. `users` holds the data the data-u / data-f indexes point into (the JS `root._users`).
fn bind_tree(root: &Element, users: Users, checked: Checked) {
    // <details> toggle events don't bubble, but they can be captured
    {
        let (users, checked) = (users.clone(), checked.clone());
        dom::keep(EventListener::new_with_options(root, "toggle", EventListenerOptions::run_in_capture_phase(), move |e: &web_sys::Event| {
            let Some(d) = dom::target_el(e) else { return };
            if !d.has_attribute("open") { return; }
            let Some(body) = dom::qs_in(&d, ":scope > [data-lazy]") else { return };
            let _ = body.remove_attribute("data-lazy");
            let i: usize = dom::data(&d, "u").parse().unwrap_or(usize::MAX);
            let users = users.borrow();
            let Some(u) = users.get(i) else { return };
            let checked = checked.borrow();
            let html = match dom::data_opt(&d, "f") {
                Some(f) => file_rows(u, arr(u, "folders").get(f.parse::<usize>().unwrap_or(usize::MAX)).unwrap_or(&Value::Null), &checked),
                None => arr(u, "folders").iter().enumerate().map(|(j, f)| folder_html(u, i, f, j, &checked, false, true)).collect(),
            };
            dom::set_html(&body, &html);
        }));
    }
    {
        let (users, checked) = (users.clone(), checked.clone());
        dom::on(root, "change", move |e| {
            let Some(cb) = dom::closest(e, "input[data-i]") else { return };
            let Some((bx, ui, fi)) = lookup(&cb) else { return };
            let users = users.borrow();
            let Some(u) = users.get(ui) else { return };
            let Some(f) = arr(u, "folders").get(fi) else { return };
            let Some(x) = arr(f, "files").get(dom::data(&cb, "i").parse::<usize>().unwrap_or(usize::MAX)) else { return };
            let key = file_key(u, x);
            { let mut c = checked.borrow_mut(); if dom::checked(&cb) { c.insert(key); } else { c.remove(&key); } }
            sync_sel(&bx, u, f, &checked.borrow());
        });
    }
    let root_el = root.clone();
    dom::on(root, "click", move |e| {
        let root = &root_el;
        let Some(b) = dom::closest(e, "[data-dl], [data-browse]") else { return };
        dom::prevent(e);   // the buttons sit inside <summary>: don't fold the card
        if let Some(name) = dom::data_opt(&b, "browse") { browse_modal(&name); return; }
        let Some((bx, ui, fi)) = lookup(&b) else { return };
        let (u, f) = {
            let users = users.borrow();
            let Some(u) = users.get(ui) else { return };
            let Some(f) = arr(u, "folders").get(fi) else { return };
            (u.clone(), f.clone())
        };
        let dl = dom::data(&b, "dl");
        let files: Vec<Value> = if dl == "folder" { arr(&f, "files").to_vec() } else {
            let c = checked.borrow();
            arr(&f, "files").iter().filter(|x| c.contains(&file_key(&u, x))).cloned().collect()
        };
        let checked = checked.clone();
        let root = root.clone();
        dom::spawn(async move {
            if download(&u, &f, &files, &b).await && dl == "selected" {
                { let mut c = checked.borrow_mut(); for x in &files { c.remove(&file_key(&u, x)); } }
                // a poll may have repainted the results meanwhile: untick the live folder, not a detached one
                let live = dom::qs_in(&root, &format!(r#"[data-u="{}"][data-f="{}"]"#, dom::data(&bx, "u"), dom::data(&bx, "f"))).unwrap_or(bx);
                for cb in dom::qsa_in(&live, "input[data-i]") { dom::set_checked(&cb, false); }
                sync_sel(&live, &u, &f, &checked.borrow());
            }
        });
    });
}

// ---- browse a user ----------------------------------------------------------------------------------

/// Wide modal with everything a user shares: GET /browse/{user}, then a filter box over the folders.
fn browse_modal(username: &str) {
    let username = username.to_string();
    dom::spawn(async move {
        let checked: Checked = Rc::new(RefCell::new(HashSet::new()));
        let m = ui::modal(ModalOpts::new(
            &format!(r#"{} <span class="faint" style="font-weight:400">· shared files</span>"#, esc(&username)),
            &format!(r#"<div class="empty"><span class="spinner"></span><p class="small muted">Asking {} for their file list – this can take a while if they are slow to answer.</p></div>"#, esc(&username)),
        ).wide(true));
        let Some(body) = dom::qs_in(&m.el, ".modal-body") else { return };
        let d = match api::get(&format!("/soulseek/browse/{}", enc(&username))).await {
            Ok(d) => d,
            Err(e) => { if m.el.is_connected() { dom::set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); } return; }
        };
        if !m.el.is_connected() { return; }
        let user = Rc::new(json!({ "username": username, "folders": d.a("folders") }));
        let total: usize = arr(&user, "folders").iter().map(|f| arr(f, "files").len()).sum();
        dom::set_html(&body, &format!(r#"<div class="row wrap" style="margin-bottom:12px">
      <div class="search grow">{}<input class="input" id="br-q" placeholder="Filter folders and files…" autocomplete="off"></div>
      <span class="small muted">{} · {}</span></div>
    <div id="br-list"></div>"#, icon("search"), plural(arr(&user, "folders").len() as f64, "folder"), plural(total as f64, "file")));
        let Some(list) = dom::qs_in(&body, "#br-list") else { return };
        bind_tree(&list, Rc::new(RefCell::new(vec![(*user).clone()])), checked.clone());
        let paint = {
            let (user, list, checked) = (user.clone(), list.clone(), checked.clone());
            move |q: &str| {
                let needle = q.to_lowercase();
                let hit = |s: &str| s.to_lowercase().contains(&needle);
                let shown: Vec<(usize, &Value)> = arr(&user, "folders").iter().enumerate()
                    .filter(|(_, f)| needle.is_empty() || hit(&f.s("directory")) || arr(f, "files").iter().any(|x| hit(&x.s("name")))).collect();
                let open = !needle.is_empty() && shown.len() <= 6;   // a narrow filter: show the files right away
                let c = checked.borrow();
                let html = if !shown.is_empty() {
                    shown.iter().take(MAX_FOLDERS).map(|(j, f)| folder_html(&user, 0, f, *j, &c, true, open)).collect::<String>()
                        + &(if shown.len() > MAX_FOLDERS { format!(r#"<div class="muted small mt">Showing {MAX_FOLDERS} of {} – narrow the filter to see the rest.</div>"#, plural(shown.len() as f64, "folder")) } else { String::new() })
                } else {
                    empty_state(&icon("search"), if !needle.is_empty() { "No folders match" } else { "Nothing shared" }, "")
                };
                dom::set_html(&list, &html);
            }
        };
        paint("");
        if let Some(q) = dom::qs_in(&body, "#br-q") {
            let q2 = q.clone();
            let deb = ui::Debounce::new(200, move || paint(&js_trim(&dom::value(&q2))));
            dom::on(&q, "input", move |_| deb.run());
        }
    });
}

// ---- transfers ----------------------------------------------------------------------------------------

fn downloads_table(rows: &[Value]) -> String {
    format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th class="hide-sm">User</th><th>File</th><th>State</th><th style="width:160px">Progress</th><th class="hide-sm">Speed / size</th><th></th></tr></thead><tbody>
    {}</tbody></table></div>"#, rows.iter().enumerate().map(|(i, x)| {
        let action = if is_active(x) { "Cancel" } else { "Remove" };
        format!(r#"<tr>
      <td class="hide-sm muted">{}</td>
      <td><div class="ellipsis" style="max-width:420px;font-weight:600" title="{}">{}</div>
        <div class="small faint ellipsis" style="max-width:420px">{}</div>
        {}</td>
      <td>{}</td>
      <td><div class="progress"><i style="width:{pct}%"></i></div><div class="small faint">{pct}%</div></td>
      <td class="hide-sm small muted" style="white-space:nowrap">{}{} / {}</td>
      <td><button class="btn sm ghost icon" data-cancel="{i}" title="{action}" aria-label="{action}">{}</button></td>
    </tr>"#,
            esc(&x.s("username")),
            esc(&x.s("filename")), esc(&if x.truthy("basename") { x.s("basename") } else { base_name(&x.s("filename")) }),
            esc(&x.s("directory")),
            if x.truthy("error") { format!(r#"<div class="small" style="color:var(--err)">{}</div>"#, esc(&x.s("error"))) } else { String::new() },
            state_badge(&x.s("state"), false),
            if is_active(x) && x.truthy("speed") { format!("{}<br>", esc(&speed_text(x.f("speed")))) } else { String::new() },
            ui::fmt::size(x.f("bytes")), ui::fmt::size(x.f("size")),
            icon("x"),
            pct = pct(x))
    }).collect::<String>())
}

fn uploads_table(rows: &[Value]) -> String {
    format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th class="hide-sm">User</th><th>File</th><th>State</th><th style="width:160px">Progress</th></tr></thead><tbody>
    {}</tbody></table></div>"#, rows.iter().map(|x| format!(r#"<tr>
      <td class="hide-sm muted">{}</td>
      <td><div class="ellipsis" style="max-width:420px;font-weight:600" title="{}">{}</div><div class="small faint">{}</div></td>
      <td>{}</td>
      <td><div class="progress"><i style="width:{}%"></i></div><div class="small faint">{} / {}</div></td>
    </tr>"#,
        esc(&x.s("username")),
        esc(&x.s("filename")), esc(&if x.truthy("basename") { x.s("basename") } else { base_name(&x.s("filename")) }), ui::fmt::size(x.f("size")),
        state_badge(&x.s("state"), true),
        pct(x), ui::fmt::size(x.f("bytes")), ui::fmt::size(x.f("size")))).collect::<String>())
}

// ---- page -----------------------------------------------------------------------------------------------

/// The JS `state` plus everything the page's closures captured.
struct Page {
    view: Element,
    status_el: Element,
    form: Element,
    s_status: Element,
    results_el: Element,
    search_el: Element,
    tr_el: Element,
    tab: RefCell<String>,
    status: RefCell<Value>,
    enabled: Cell<bool>,
    forbidden: Cell<bool>,
    results: Users,
    transfers: RefCell<Value>,
    checked: Checked,   // "user\nfilename" of every ticked file in the search results
    gen: Cell<u64>,
    poll_t: RefCell<Option<Timeout>>,
}

/// `ctx.onLeave(() => { gen++; clearTimeout(pollT); })`.
struct LeaveGuard(Rc<Page>);
impl Drop for LeaveGuard {
    fn drop(&mut self) { self.0.gen.set(self.0.gen.get() + 1); self.0.poll_t.borrow_mut().take(); }
}

impl Page {
    /// The status line.
    async fn load_status(&self) {
        match api::get("/soulseek/status").await {
            Ok(s) => {
                self.enabled.set(s.truthy("enabled"));
                dom::set_html(&self.status_el, &status_html(&s, ""));
                *self.status.borrow_mut() = s;
            }
            Err(e) => {
                self.enabled.set(!matches!(e.status, 400 | 403));   // 400 "not enabled", 403 may not pick downloads
                self.forbidden.set(e.status == 403);
                dom::set_html(&self.status_el, &status_html(&Value::Null, &e.message));
            }
        }
        dom::toggle_class(&self.form, "hidden", !self.enabled.get());
        if !self.enabled.get() {
            dom::set_html(&self.s_status, "");
            dom::set_html(&self.results_el, &if self.forbidden.get() {
                empty_state(&icon("users"), "Soulseek is for people who may pick downloads", "Ask an admin to allow manual download picking for your account.")
            } else {
                empty_state(&icon("users"), "Soulseek is unavailable", r##"An admin can turn it on under <a href="#/settings?s=slskd" style="color:var(--accent)">Settings → Soulseek</a>."##)
            });
        }
    }

    fn paint_results(&self, done: bool) {
        let mut open: HashSet<String> = dom::qsa_in(&self.results_el, "details.slsk-user[open]").iter().map(|d| dom::data(d, "user")).collect();
        let results = self.results.borrow();
        if done && open.is_empty() { if let Some(first) = results.first() { open.insert(first.s("username")); } }
        let checked = self.checked.borrow();
        let shown = &results[..results.len().min(MAX_USERS)];
        let html = shown.iter().enumerate().map(|(i, u)| user_card(u, i, &checked, open.contains(&u.s("username")))).collect::<String>()
            + &(if results.len() > shown.len() { format!(r#"<div class="muted small">Showing the first {MAX_USERS} of {} – free slots and fast uploaders come first.</div>"#, plural(results.len() as f64, "user")) } else { String::new() });
        dom::set_html(&self.results_el, &html);
    }

    /// search: POST /search, then GET /search/{id} once a second until done
    fn search(self: &Rc<Self>, q: String) {
        let my = self.gen.get() + 1;
        self.gen.set(my);
        self.poll_t.borrow_mut().take();
        self.results.borrow_mut().clear();
        self.checked.borrow_mut().clear();
        dom::set_html(&self.results_el, "");
        dom::set_html(&self.s_status, r#"<span class="spinner" style="width:12px;height:12px"></span> Starting the search…"#);
        let p = self.clone();
        dom::spawn(async move {
            let s = match api::post("/soulseek/search", json!({ "q": q })).await {
                Ok(s) => s,
                Err(e) => { if my == p.gen.get() { dom::set_html(&p.s_status, &format!(r#"<span style="color:var(--err)">{}</span>"#, esc(&e.message))); } return; }
            };
            p.poll(my, Rc::new(s), Rc::new(q));
        });
    }

    fn poll(self: &Rc<Self>, my: u64, s: Rc<Value>, q: Rc<String>) {
        if my != self.gen.get() || !self.view.is_connected() { return; }
        let p = self.clone();
        dom::spawn(async move {
            let r = match api::get(&format!("/soulseek/search/{}", enc(&s.s("id")))).await {
                Ok(r) => r,
                Err(e) => {
                    if my == p.gen.get() {
                        dom::set_html(&p.s_status, &format!(r#"<span style="color:var(--err)">{}</span>"#, esc(if e.status == 404 { "That search has expired – run it again." } else { &e.message })));
                    }
                    return;
                }
            };
            if my != p.gen.get() { return; }
            *p.results.borrow_mut() = sort_users(r.a("results"));
            let (n, files) = {
                let results = p.results.borrow();
                (results.len(), results.iter().map(|u| arr(u, "folders").iter().map(|f| arr(f, "files").len()).sum::<usize>()).sum::<usize>())
            };
            p.paint_results(r.truthy("done"));
            if r.truthy("done") {
                dom::set_html(&p.s_status, &if n > 0 {
                    format!("{} · {} for “{}”", plural(n as f64, "user"), plural(files as f64, "file"), esc(&if r.truthy("q") { r.s("q") } else { (*q).clone() }))
                } else { String::new() });
                if n == 0 { dom::set_html(&p.results_el, &empty_state(&icon("search"), "Nothing found", "Try fewer words, or just the artist and album – Soulseek matches file and folder names.")); }
            } else {
                dom::set_html(&p.s_status, &format!(r#"<span class="spinner" style="width:12px;height:12px"></span> Searching for up to {} s… {} so far"#,
                    if s.truthy("seconds") { esc(&s.s("seconds")) } else { "?".to_string() }, plural(n as f64, "user")));
                let (p2, s2, q2) = (p.clone(), s.clone(), q.clone());
                *p.poll_t.borrow_mut() = Some(Timeout::new(1000, move || p2.poll(my, s2, q2)));
            }
        });
    }

    /// transfers: repainted from state every 2 s while the tab is open; the tab label counts active downloads
    fn paint_transfers(&self) {
        let t = self.transfers.borrow();
        let (dl, ul) = (arr(&t, "downloads"), arr(&t, "uploads"));
        dom::set_html(&self.tr_el, &format!(r#"
      <div class="section-head"><h2>Downloads <span class="faint small">{}</span></h2>
        <button class="btn sm" id="slsk-clear" {}>{}Clear finished</button></div>
      <div class="card flat">{}</div>
      <div class="section-head"><h2>Uploads <span class="faint small">{}</span></h2></div>
      <div class="card flat slsk-ul">{}</div>"#,
            dl.len(), if dl.iter().any(is_finished) { "" } else { "disabled" }, icon("trash"),
            if !dl.is_empty() { downloads_table(dl) } else { empty_state(&icon("download"), "No downloads", "Files you download from Soulseek show up here while they transfer.") },
            ul.len(),
            if !ul.is_empty() { uploads_table(ul) } else {
                empty_state(&icon("users"), "No uploads", "Other people fetching from your shares show up here.")
            }));
    }

    async fn load_transfers(&self) {
        match api::get("/soulseek/transfers").await {
            Ok(t) => *self.transfers.borrow_mut() = t,
            Err(e) => {
                if *self.tab.borrow() == "transfers" && dom::qs_in(&self.tr_el, ".table").is_none() {
                    dom::set_html(&self.tr_el, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message)));
                }
                return;
            }
        }
        let active = arr(&self.transfers.borrow(), "downloads").iter().filter(|x| is_active(x)).count();
        if let Some(c) = dom::qs_in(&self.view, "#slsk-tcount") { dom::set_text(&c, &if active > 0 { active.to_string() } else { String::new() }); }
        if *self.tab.borrow() == "transfers" { self.paint_transfers(); }
    }

    fn show_tab(self: &Rc<Self>, t: &str) {
        *self.tab.borrow_mut() = t.to_string();
        for b in dom::qsa_in(&self.view, "#slsk-tabs button") { dom::toggle_class(&b, "active", dom::data_opt(&b, "t").as_deref() == Some(t)); }
        dom::toggle_class(&self.search_el, "hidden", t != "search");
        dom::toggle_class(&self.tr_el, "hidden", t != "transfers");
        if t == "transfers" {
            self.paint_transfers();
            if self.enabled.get() { let p = self.clone(); dom::spawn(async move { p.load_transfers().await }); }
        }
    }
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let tab = if ctx.q("tab") == "transfers" { "transfers" } else { "search" };
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Soulseek</h1><div class="small muted slsk-status" id="slsk-status"><span class="spinner" style="width:12px;height:12px"></span></div></div>
      <div class="tabs" id="slsk-tabs">
        <button data-t="search" class="{}">Search</button>
        <button data-t="transfers" class="{}">Transfers <span class="faint" id="slsk-tcount"></span></button>
      </div>
    </div>
    <div id="slsk-search" class="{}">
      <form class="row wrap" id="slsk-form">
        <div class="search grow">{search}<input class="input" name="q" type="search" enterkeyhint="search" placeholder="Artist, album, song – the way the uploader probably named it" value="{}" autocomplete="off"></div>
        <button class="btn primary" type="submit">{search}Search</button>
      </form>
      <div class="small muted mt" id="slsk-sstatus"></div>
      <div class="stack" id="slsk-results" style="margin-top:10px"></div>
    </div>
    <div id="slsk-transfers" class="{}"></div>"#,
            if tab == "search" { "active" } else { "" },
            if tab == "transfers" { "active" } else { "" },
            if tab == "search" { "" } else { "hidden" },
            esc(&ctx.q("q")),
            if tab == "transfers" { "" } else { "hidden" },
            search = icon("search")));
        let page = Rc::new(Page {
            status_el: need(&view, "#slsk-status")?,
            form: need(&view, "#slsk-form")?,
            s_status: need(&view, "#slsk-sstatus")?,
            results_el: need(&view, "#slsk-results")?,
            search_el: need(&view, "#slsk-search")?,
            tr_el: need(&view, "#slsk-transfers")?,
            view: view.clone(),
            tab: RefCell::new(tab.to_string()),
            status: RefCell::new(Value::Null),
            enabled: Cell::new(true),
            forbidden: Cell::new(false),
            results: Rc::new(RefCell::new(Vec::new())),
            transfers: RefCell::new(json!({ "downloads": [], "uploads": [] })),
            checked: Rc::new(RefCell::new(HashSet::new())),
            gen: Cell::new(0),
            poll_t: RefCell::new(None),
        });
        bind_tree(&page.results_el, page.results.clone(), page.checked.clone());

        ctx.on_leave(LeaveGuard(page.clone()));
        {
            let p = page.clone();
            dom::on(&page.form, "submit", move |e| {
                dom::prevent(e);
                let q = js_trim(&dom::value_of(&p.form, "[name=q]"));
                if q.is_empty() { if let Some(i) = dom::qs_in(&p.form, "input") { dom::focus(&i); } return; }
                p.search(q);
            });
        }

        {
            let p = page.clone();
            dom::on(&page.tr_el, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-cancel], #slsk-clear") else { return };
                dom::set_disabled(&b, true);
                let p = p.clone();
                dom::spawn(async move {
                    let r = match dom::data_opt(&b, "cancel") {
                        Some(i) => {
                            let x = arr(&p.transfers.borrow(), "downloads").get(i.parse::<usize>().unwrap_or(usize::MAX)).cloned();
                            let Some(x) = x else { dom::set_disabled(&b, false); return };
                            api::del(&format!("/soulseek/transfers/{}/{}", enc(&x.s("username")), enc(&x.s("id")))).await
                        }
                        None => api::post_empty("/soulseek/transfers/clear").await,
                    };
                    match r {
                        Ok(_) => p.load_transfers().await,
                        Err(err) => { toast(&err.message, "err"); dom::set_disabled(&b, false); }
                    }
                });
            });
        }

        // tabs
        {
            let p = page.clone();
            dom::on_sel(&view, "#slsk-tabs", "click", move |e| { if let Some(b) = dom::closest(e, "[data-t]") { p.show_tab(&dom::data(&b, "t")); } });
        }

        page.load_status().await;
        if page.enabled.get() {
            page.load_transfers().await;
            let q = ctx.q("q");
            if !q.is_empty() { page.search(q); }
        }
        if *page.tab.borrow() == "transfers" { page.paint_transfers(); }
        {
            let p = page.clone();
            let mut n: u64 = 0;
            dom::interval(&view, 2000, move || {
                if !p.view.is_connected() { return; }
                n += 1;
                if n % 15 == 0 { let p = p.clone(); dom::spawn(async move { p.load_status().await }); }                                                    // every 30 s
                if p.enabled.get() && (*p.tab.borrow() == "transfers" || n % 5 == 0) { let p = p.clone(); dom::spawn(async move { p.load_transfers().await }); }   // 2 s on the tab, 10 s for the badge
            });
        }
        Ok(())
    })
}
