//! Port of web/js/pages/diagnostics.js – see docs/ui-rewrite/CONTRACT.md
use crate::api::{self, ApiError};
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, truthy, J};
use crate::ui::{confirm_dialog, copy_text, esc, esck, fmt, icon, toast};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, FormData, HtmlInputElement};

const KIND: [(&str, &str); 3] = [("service", "Connections"), ("path", "Folders & permissions"), ("tool", "Tools in the container")];

/// `${obj[key]}` / `String(obj[key])` as JavaScript prints it (null → "null", missing → "undefined").
fn js_str(v: &Value, key: &str) -> String {
    if !v.has(key) { return "undefined".into(); }
    match v.g(key) { Value::Null => "null".into(), x => js_string(x) }
}
/// `String(obj[key] || '')`.
fn or_empty(v: &Value, key: &str) -> String { if v.truthy(key) { v.s(key) } else { String::new() } }

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Diagnostics</h1><div class="small muted">Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.</div></div>
      <div class="row"><button class="btn" id="copy" disabled>{copy}Copy report</button><button class="btn primary" id="run">{refresh}Run checks</button></div>
    </div>
    <div id="diag"><div class="empty"><span class="spinner"></span></div></div>
    <div class="section-head"><h2>Backups</h2>
      <div class="row"><label class="btn sm">{up}Upload a backup…<input type="file" id="bk-upload" accept=".db,.sqlite,.sqlite3,application/vnd.sqlite3,application/x-sqlite3" hidden></label><button class="btn sm primary" id="bk-now">Back up now</button></div>
    </div>
    <div class="card flat">
      <div class="small muted" style="margin-bottom:10px">Encore keeps copies of its database here (daily, and before every upgrade). Download one now and then to keep it somewhere else.</div>
      <div id="backups"><div class="empty"><span class="spinner"></span></div></div>
    </div>"#, copy = icon("copy"), refresh = icon("refresh"),
            // `icon.upload || icon.plus`
            up = { let u = icon("upload"); if u.is_empty() { icon("plus") } else { u } }));
        let Some(diag) = dom::qs_in(&view, "#diag") else { return Err("#diag is missing".into()) };
        let Some(bk) = dom::qs_in(&view, "#backups") else { return Err("#backups is missing".into()) };
        {
            let bk2 = bk.clone();
            dom::on_sel(&view, "#bk-now", "click", move |e| {
                let Some(btn) = dom::target_el(e) else { return };
                let bk = bk2.clone();
                dom::spawn(async move {
                    dom::set_disabled(&btn, true);
                    match api::post_empty("/system/backups").await {
                        Ok(d) => {
                            toast(&format!("Backed up to backups/{}", d.g("created").s("name")), "ok");
                            draw_backups(&bk, &d);
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&btn, false);
                });
            });
        }
        {
            let bk2 = bk.clone();
            dom::on_sel(&view, "#bk-upload", "change", move |e| {
                let Some(input) = dom::target_el(e).and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else { return };
                let Some(f) = input.files().and_then(|l| l.get(0)) else { return };
                input.set_value("");
                let bk = bk2.clone();
                dom::spawn(async move {
                    dom::set_html(&bk, r#"<div class="empty"><span class="spinner"></span><p class="muted">Uploading and checking…</p></div>"#);
                    let Ok(fd) = FormData::new() else { return };
                    let _ = fd.append_with_blob("file", &f);
                    let req = gloo_net::http::RequestBuilder::new("/api/system/backups/upload")
                        .method(gloo_net::http::Method::POST)
                        .header("X-Requested-With", "encore")
                        .credentials(web_sys::RequestCredentials::SameOrigin)
                        .body(fd);
                    let Ok(req) = req else { return };
                    let Ok(r) = req.send().await else { return };
                    if !r.ok() {
                        // (await r.json().catch(() => ({}))).detail || 'Upload failed'
                        let data: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
                        let detail = data.g("detail");
                        toast(&if truthy(detail) { js_string(detail) } else { "Upload failed".to_string() }, "err");
                        load_backups(&bk).await;
                        return;
                    }
                    let d: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
                    toast(&format!("Saved as backups/{}", d.g("created").s("name")), "ok");
                    draw_backups(&bk, &d);
                });
            });
        }
        {
            let bk = bk.clone();
            dom::spawn(async move { load_backups(&bk).await });
        }
        let last: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        {
            let (v, d, l) = (view.clone(), diag.clone(), last.clone());
            dom::on_sel(&view, "#run", "click", move |_| {
                let (v, d, l) = (v.clone(), d.clone(), l.clone());
                // the JS `onclick = run`: a failure here is an unhandled rejection (console only)
                dom::spawn(async move { if let Err(e) = run(v, d, l).await { dom::console_error(&e.message); } });
            });
        }
        {
            let l = last.clone();
            dom::on_sel(&view, "#copy", "click", move |_| {
                let l = l.borrow();
                let Some(last) = l.as_ref() else { return };
                copy_report(last);
            });
        }
        run(view, diag, last).await.map_err(|e| e.message)?;
        Ok(())
    })
}

/// The JS `drawBackups`: the Backups table with Download / Restore… per row.
fn draw_backups(bk: &Element, d: &Value) {
    let list = d.a("backups");
    if list.is_empty() {
        dom::set_html(bk, r#"<div class="empty muted small">No backups yet – the first one is taken a day after the first start, or press Back up now.</div>"#);
    } else {
        let rows: String = list.iter().map(|b| format!(r#"<tr><td class="mono small">{name}</td><td class="small">{size}</td><td class="small">{date}</td><td><span class="badge">{kind}</span></td>
        <td><div class="row" style="justify-content:flex-end;gap:6px"><a class="btn sm" href="/api/system/backups/{href}" download="{name}">{dl}Download</a><button class="btn sm" data-restore="{name}">Restore…</button></div></td></tr>"#,
            name = esck(b, "name"), size = fmt::size(b.f("size")), date = fmt::date(b.f("time")), kind = esck(b, "kind"),
            href = String::from(js_sys::encode_uri_component(&b.s("name"))), dl = icon("download"))).collect();
        dom::set_html(bk, &format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th>Name</th><th>Size</th><th>Date</th><th>Kind</th><th></th></tr></thead><tbody>
      {rows}
    </tbody></table></div>"#));
    }
    let bk2 = bk.clone();
    dom::on_each(bk, "[data-restore]", "click", move |el, _| {
        let name = dom::data(el, "restore");
        let bk = bk2.clone();
        dom::spawn(async move {
            let ok = confirm_dialog("Restore this backup?", &format!("Encore replaces its current database with {name}. A safety copy of the current data is taken first, then the server reopens the database: everything saved since that backup (library index, requests, playlists, settings and sign-ins) is replaced, and this page reloads – you may have to sign in again."), "Restore", true).await;
            if !ok { return; }
            dom::set_html(&bk, r#"<div class="empty"><span class="spinner"></span><p class="muted">Restoring – the server reopens its database, this page reloads in a moment…</p></div>"#);
            if let Err(e) = api::post_empty(&format!("/system/backups/{}/restore", String::from(js_sys::encode_uri_component(&name)))).await {
                toast(&e.message, "err");
                load_backups(&bk).await;
                return;
            }
            toast("Backup restored", "ok");
            dom::timeout(2500, dom::reload);
        });
    });
}

/// The JS `loadBackups`.
async fn load_backups(bk: &Element) {
    match api::get("/system/backups").await {
        Ok(d) => draw_backups(bk, &d),
        Err(e) => dom::set_html(bk, &format!(r#"<div class="empty muted small">{}</div>"#, esc(&e.message))),
    }
}

/// The JS `run`: fetch the checks and draw them into `#diag` (the result is kept in `last` for the report).
async fn run(view: Element, diag: Element, last: Rc<RefCell<Option<Value>>>) -> Result<(), ApiError> {
    dom::set_html(&diag, r#"<div class="empty"><span class="spinner"></span><p class="muted">Checking…</p></div>"#);
    let data = api::get("/system/diagnostics").await?;
    *last.borrow_mut() = Some(data.clone());
    if let Some(c) = dom::qs_in(&view, "#copy") { dom::set_disabled(&c, false); }
    let checks = data.a("checks");
    let bad = checks.iter().filter(|c| !c.truthy("ok")).count();
    let groups: String = KIND.iter().map(|(k, title)| {
        let g: Vec<&Value> = checks.iter().filter(|c| c.s("kind") == *k).collect();
        if g.is_empty() { return String::new(); }
        let items: String = g.iter().map(|c| check_item(c)).collect();
        format!(r#"
        <div class="section-head"><h2>{title}</h2></div>
        <div class="card flat"><div class="list">{items}</div></div>"#)
    }).collect();
    let rows: String = data.a("settings").iter().map(|s| format!(r#"<tr><td class="mono small">{key}</td>
          <td>{source}</td>
          <td class="small mono ellipsis" style="max-width:360px">{value}</td></tr>"#,
        key = esck(s, "key"),
        source = if s.g("source").as_str() == Some("env") { format!(r#"<span class="badge info">env {}</span>"#, esck(s, "env")) } else { r#"<span class="badge">saved in Encore</span>"#.to_string() },
        value = esc(&js_str(s, "value")),
    )).collect();
    dom::set_html(&diag, &format!(r#"
      <div class="card row" style="margin-bottom:16px">
        <span class="badge {badc}">{badl}</span>
        <span class="muted small">Encore {version} · running as uid {uid} · {time}</span>
      </div>
      {groups}
      <div class="section-head"><h2>Where settings come from</h2><span class="small muted">Environment variables always win over values saved in Encore</span></div>
      <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>Setting</th><th>Source</th><th>Value</th></tr></thead><tbody>
        {rows}
      </tbody></table></div></div>"#,
        badc = if bad > 0 { "err" } else { "ok" },
        badl = if bad > 0 { format!("{bad} problem{}", if bad > 1 { "s" } else { "" }) } else { "All good".to_string() },
        version = esck(&data, "version"),
        uid = js_str(&data, "uid"),
        time = fmt::date(data.f("time")),
    ));
    Ok(())
}

/// One `.list-item` of a check group.
fn check_item(c: &Value) -> String {
    let ok = c.truthy("ok");
    let okc = if ok { "ok" } else { "err" };
    let status = if c.truthy("status") { format!(r#"<span class="badge {okc}">HTTP {}</span>"#, js_str(c, "status")) } else { String::new() };
    let ms = if !c.is_nullish("ms") { format!(r#"<span class="small faint">{} ms</span>"#, js_str(c, "ms")) } else { String::new() };
    let body = if c.truthy("body") {
        format!(r#"<details><summary class="small muted" style="cursor:pointer">Raw response</summary><div class="code diag-raw">{}</div></details>"#, esck(c, "body"))
    } else { String::new() };
    format!(r#"
          <div class="list-item" style="align-items:flex-start">
            <span class="dot {okc}" style="margin-top:7px"></span>
            <div class="grow">
              <div class="row wrap"><b>{name}</b>{status}{ms}</div>
              <div class="small mono faint ellipsis">{request}</div>
              <div class="small" style="color:{color}">{detail}</div>
              {body}
            </div>
          </div>"#,
        name = esck(c, "name"),
        request = esc(&or_empty(c, "request")),
        color = if ok { "var(--muted)" } else { "var(--err)" },
        detail = esc(&or_empty(c, "detail")),
    )
}

/// The `#copy` handler: the plain-text report on the clipboard.
fn copy_report(last: &Value) {
    let mut lines = vec![format!("Encore {} diagnostics (uid {})", js_str(last, "version"), js_str(last, "uid"))];
    for c in last.a("checks") {
        let ok = c.truthy("ok");
        let status = if c.truthy("status") { format!(" → HTTP {}", js_str(&c, "status")) } else { String::new() };
        let ms = if !c.is_nullish("ms") { format!(" ({} ms)", js_str(&c, "ms")) } else { String::new() };
        // `c.body.slice(0, 300)` – JS slices by UTF-16 units
        let body = if c.truthy("body") && !ok { format!("\n     body: {}", String::from(js_sys::JsString::from(c.s("body")).slice(0, 300))) } else { String::new() };
        lines.push(format!("{} {} – {}{status}{ms}\n     {}{body}", if ok { "OK  " } else { "FAIL" }, js_str(&c, "name"), or_empty(&c, "request"), or_empty(&c, "detail")));
    }
    lines.push(String::new());
    lines.push("Settings:".into());
    for s in last.a("settings") {
        lines.push(format!("  {} = {}  [{}]", js_str(&s, "key"), js_str(&s, "value"), if s.g("source").as_str() == Some("env") { format!("env {}", js_str(&s, "env")) } else { "saved".to_string() }));
    }
    copy_text(&lines.join("\n"));
}
