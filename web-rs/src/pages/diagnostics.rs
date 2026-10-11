//! Port of web/js/pages/diagnostics.js – see docs/ui-rewrite/CONTRACT.md
use crate::api::{self, ApiError};
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, J};
use crate::ui::{copy_text, esc, esck, fmt, icon};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

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
    <div id="diag"><div class="empty"><span class="spinner"></span></div></div>"#, copy = icon("copy"), refresh = icon("refresh")));
        let Some(diag) = dom::qs_in(&view, "#diag") else { return Err("#diag is missing".into()) };
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
