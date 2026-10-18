//! Port of web/js/pages/activity.js – see docs/archive/ui-rewrite/CONTRACT.md
//!
//! The download activity list: Soulseek transfers, torrent/Usenet downloads and the activity log,
//! reloaded every 5 seconds while the page is on screen.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{empty_state, esc, fmt, icon, toast};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, &format!(r#"
    <h1 style="margin-bottom:18px">Activity</h1>
    <div class="section-head"><h2>Soulseek transfers</h2><button class="btn sm" id="clear-slskd">{}Clear completed</button></div>
    <div class="card flat" id="slskd"><div class="empty"><span class="spinner"></span></div></div>
    <div class="section-head"><h2>Torrents & Usenet</h2></div>
    <div class="card flat" id="torrents"></div>
    <div class="section-head"><h2>Log</h2>
      <select class="input" id="lvl" style="width:auto"><option value="">All levels</option><option value="info">Info</option><option value="warning">Warnings</option><option value="error">Errors</option></select></div>
    <div class="card flat" id="log"></div>"#, icon("trash")));
        let level = Rc::new(RefCell::new(String::new()));
        {
            let (v, l) = (view.clone(), level.clone());
            dom::on_sel(&view, "#lvl", "change", move |e| {
                *l.borrow_mut() = dom::target_el(e).map(|t| dom::value(&t)).unwrap_or_default();
                dom::spawn(load_quietly(v.clone(), l.clone()));
            });
        }
        {
            let (v, l) = (view.clone(), level.clone());
            dom::on_sel(&view, "#clear-slskd", "click", move |_| {
                let (v, l) = (v.clone(), l.clone());
                dom::spawn(async move {
                    match api::post_empty("/system/downloads/slskd/clear").await {
                        Ok(_) => dom::spawn(load_quietly(v, l)),
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            });
        }
        load(view.clone(), level.clone()).await?;
        let (v, l) = (view.clone(), level);
        dom::interval(&view, 5000, move || dom::spawn(load_quietly(v.clone(), l.clone())));
        Ok(())
    })
}

/// The JS `load`: both lists at once (`Promise.all`), then the three cards. `level` is read after the
/// requests, when the log is filtered, exactly like the closure variable in the JS.
async fn load(view: Element, level: Rc<RefCell<String>>) -> Result<(), String> {
    let (d, logs) = futures::future::try_join(api::get("/system/downloads"), api::get("/system/activity")).await.map_err(|e| e.message)?;
    let html = if d.g("errors").truthy("slskd") {
        format!(r#"<div class="error-box" style="margin:14px">{}</div>"#, esc(&d.g("errors").s("slskd")))
    } else {
        let files = d.a("slskd");
        if !files.is_empty() {
            format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th>File</th><th class="hide-sm">User</th><th>State</th><th style="width:180px">Progress</th></tr></thead><tbody>
        {}
        </tbody></table></div>"#, files.iter().take(200).map(slskd_row).collect::<String>())
        } else {
            empty_state(&icon("download"), "No Soulseek transfers", "slskd may not be configured.")
        }
    };
    set_in(&view, "#slskd", &html);
    let tor = d.a("torrents");
    let html = if !tor.is_empty() {
        format!(r#"<div class="list">{}</div>"#, tor.iter().map(torrent_row).collect::<String>())
    } else {
        empty_state(&icon("download"), "Nothing downloading from torrents or Usenet", "")
    };
    set_in(&view, "#torrents", &html);
    let level = level.borrow().clone();
    let rows: Vec<Value> = logs.items().into_iter().filter(|l| level.is_empty() || l.g("level").as_str() == Some(level.as_str())).collect();
    let html = if !rows.is_empty() {
        format!(r#"<div class="list">{}</div>"#, rows.iter().take(200).map(log_row).collect::<String>())
    } else {
        r#"<div class="empty muted">No log entries</div>"#.to_string()
    };
    set_in(&view, "#log", &html);
    Ok(())
}

/// `load()` called without `await`: the JS let a failure go as an unhandled rejection, so the result is dropped.
async fn load_quietly(view: Element, level: Rc<RefCell<String>>) { let _ = load(view, level).await; }

/// `view.querySelector(sel).innerHTML = html`.
fn set_in(view: &Element, sel: &str, html: &str) {
    if let Some(el) = dom::qs_in(view, sel) { dom::set_html(&el, html); }
}

/// One `<tr>` of the Soulseek table (the `d.slskd.slice(0, 200).map(f => …)` row).
fn slskd_row(f: &Value) -> String {
    let state = f.s("state");
    let cls = if state.contains("Succeeded") { "ok" }
        else if ["Errored", "Rejected", "TimedOut", "Cancelled"].iter().any(|k| state.contains(k)) { "err" }
        else { "info" };
    let speed = if f.truthy("speed") { format!(" · {}/s", fmt::size(f.f("speed"))) } else { String::new() };
    format!(r#"<tr><td><div class="ellipsis" style="max-width:420px;font-weight:600">{}</div><div class="small faint ellipsis" style="max-width:420px">{} · {}</div></td>
          <td class="hide-sm muted">{}</td>
          <td><span class="badge {}">{}</span></td>
          <td><div class="progress"><i style="width:{}%"></i></div><div class="small faint">{}%{}</div></td></tr>"#,
        esc(&f.s("filename")), esc(&f.s("directory")), fmt::size(f.f("size")), esc(&f.s("username")), cls, esc(&state), f.s("progress"), f.s("progress"), speed)
}

/// One `.list-item` of the torrents/Usenet list (the `tor.map(q => …)` row).
fn torrent_row(q: &Value) -> String {
    let message = if q.truthy("message") { format!(r#"<div class="small muted">{}</div>"#, esc(&q.s("message"))) } else { String::new() };
    format!(r#"<div class="list-item"><div class="grow"><b>{} – {}</b>
          {}
          <div class="progress" style="margin-top:6px;max-width:420px"><i style="width:{}%"></i></div></div></div>"#,
        esc(&q.s("artist")), esc(&q.s("title")), message, q.f("progress").round() as i64)
}

/// One `.list-item` of the log (the `rows.slice(0, 200).map(l => …)` row).
fn log_row(l: &Value) -> String {
    let level = l.s("level");
    let cls = if level == "error" { "err" } else if level == "warning" { "warn" } else { "" };
    format!(r#"<div class="list-item" style="padding:8px 16px">
      <span class="badge {}" style="width:68px;justify-content:center">{}</span>
      <span class="grow small">{}</span><span class="small faint" title="{}">{}</span></div>"#,
        cls, esc(&level), esc(&l.s("message")), fmt::date(l.f("ts")), fmt::ago(l.f("ts")))
}
