//! Port of web/js/pages/settings.js (renderTasks) – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → Scheduled tasks: every background job, with on/off, interval, last result and Run now, and how long
//! finished downloads are kept. The page's settings card (Playlists & sync) comes from the schema under it.

use crate::api;
use crate::dom;
use crate::json::{js_string, json, J};
use crate::ui::{esc, icon, toast};
use serde_json::Value;
use web_sys::Element;

const EVERY: &[(i64, &str)] = &[(15, "15 minutes"), (30, "30 minutes"), (60, "hour"), (180, "3 hours"), (360, "6 hours"), (720, "12 hours"), (1440, "day"), (10080, "week")];
/// `Math.round(x)` (halves round towards +∞).
fn js_round(x: f64) -> i64 { (x + 0.5).floor() as i64 }
/// `${n}` of a JS number (15 → "15", 2.5 → "2.5").
fn js_num(n: f64) -> String { js_string(&Value::from(n)) }
/// JS unary `+value` on an input's string, as JSON: a number, or null for NaN.
fn js_plus(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() { return Value::from(0); }
    match t.parse::<f64>() {
        Ok(f) if f.is_finite() => if f.fract() == 0.0 && f.abs() < 9.0e15 { Value::from(f as i64) } else { Value::from(f) },
        _ => Value::Null,
    }
}
fn ago(t: f64) -> String {
    if t == 0.0 { return "never".into(); }
    let s = dom::now_s() - t;
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", js_round(s / 60.0)) } else if s < 129600.0 { format!("{} h ago", js_round(s / 3600.0)) } else { format!("{} days ago", js_round(s / 86400.0)) }
}
fn until(t: f64) -> String {
    if t == 0.0 { return String::new(); }
    let s = t - dom::now_s();
    if s <= 60.0 { "any moment".into() } else if s < 5400.0 { format!("in {} min", js_round(s / 60.0)) } else if s < 129600.0 { format!("in {} h", js_round(s / 3600.0)) } else { format!("in {} days", js_round(s / 86400.0)) }
}

fn task_row(t: &Value) -> String {
    let locked = t.truthy("locked");
    let enabled = t.truthy("enabled");
    let running = t.truthy("running");
    let minutes = t.f("minutes");
    // [...new Set([...EVERY.map(e => e[0]), t.minutes])].sort((a, b) => a - b)
    let mut ms: Vec<f64> = EVERY.iter().map(|e| e.0 as f64).collect();
    if !ms.contains(&minutes) { ms.push(minutes); }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let every: String = ms.iter().map(|m| {
        let lab = EVERY.iter().find(|e| e.0 as f64 == *m).map(|e| e.1.to_string()).unwrap_or_else(|| format!("{} minutes", js_num(*m)));
        format!(r##"<option value="{}" {}>{}</option>"##, js_num(*m), if *m == minutes { "selected" } else { "" }, lab)
    }).collect();
    let took = if t.truthy("duration") {
        let d = t.f("duration");
        format!(" · took {}", if d < 60.0 { format!("{} s", js_round(d)) } else { format!("{} min", js_round(d / 60.0)) })
    } else { String::new() };
    let run_note = if running { r##" · <b style="color:var(--accent)">running…</b>"## } else { "" };
    let result = if t.truthy("error") { format!(r##" · <span style="color:var(--err)">{}</span>"##, esc(&t.s("error"))) }
        else if t.truthy("result") && !running { format!(" · {}", esc(&t.s("result"))) } else { String::new() };
    let next = if enabled && t.truthy("next_run") && !running { format!(" · next {}", until(t.f("next_run"))) } else { String::new() };
    format!(r##"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-k="{key}">
      <label class="switch" style="margin-top:2px" title="{ltitle}"><input type="checkbox" data-on {checked} {ldis}><i></i></label>
      <div class="grow" style="min-width:220px"><div style="font-weight:650">{name}{lock}</div>
        <div class="small muted">{desc}</div>
        <div class="small faint" style="margin-top:4px">Last run {ago}{took}{run_note}
          {result}{next}</div></div>
      <div class="row" style="gap:8px"><span class="small muted">every</span>
        <select class="input" data-every style="width:auto" {ldis}>{every}</select>
        <button class="btn sm" data-run {rdis}>{play}Run now</button></div>
    </div>"##,
        key = t.s("key"),
        ltitle = if locked { "Set by an environment variable" } else { "" },
        checked = if enabled { "checked" } else { "" },
        ldis = if locked { "disabled" } else { "" },
        name = esc(&t.s("name")),
        lock = if locked { r##" <span title="Set by an environment variable">🔒</span>"## } else { "" },
        desc = esc(&t.s("desc")),
        ago = ago(t.f("last_run")),
        rdis = if running { "disabled" } else { "" },
        play = icon("play"))
}

/// Settings → Scheduled tasks: every background job, with on/off, interval, last result and Run now.
pub async fn render_tasks(panel: Element) {
    let d = match api::get("/admin/tasks").await { Ok(d) => d, Err(e) => { dom::console_error(&e.message); return; } };
    let rows: String = d.a("tasks").iter().map(task_row).collect();
    dom::set_html(&panel, &format!(r##"<div class="card"><div class="row between" style="margin-bottom:8px"><h2>Scheduled tasks</h2><button class="btn sm" id="t-refresh">{refresh}Refresh</button></div>
    <p class="muted small" style="margin:0 0 14px">Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now.</p>
    <div class="list">{rows}</div>
    <div class="row mt" style="gap:8px"><span class="small muted">Clean up finished downloads older than</span>
      <input class="input" type="number" min="1" id="t-days" value="{days}" style="width:90px"><span class="small muted">days</span></div></div>"##,
        refresh = icon("refresh"), days = js_string(d.g("cleanup_days"))));
    if let Some(b) = dom::qs_in(&panel, "#t-refresh") {
        let p = panel.clone();
        dom::on(&b, "click", move |_| dom::spawn(render_tasks(p.clone())));
    }
    if let Some(i) = dom::qs_in(&panel, "#t-days") {
        let i2 = i.clone();
        dom::on(&i, "change", move |_| {
            let days = js_plus(&dom::value(&i2));
            dom::spawn(async move {
                match api::put("/admin/settings", json!({ "task_cleanup_days": days })).await {
                    Ok(_) => toast("Saved", "ok"),
                    Err(e) => dom::console_error(&e.message),
                }
            });
        });
    }
    for row in dom::qsa_in(&panel, "[data-k]") {
        let k = dom::data(&row, "k");
        if let Some(on) = dom::qs_in(&row, "[data-on]") {
            let (on2, k2, p) = (on.clone(), k.clone(), panel.clone());
            dom::on(&on, "change", move |_| {
                let checked = dom::checked(&on2);
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::patch(&format!("/admin/tasks/{k}"), json!({ "enabled": checked })).await {
                        Ok(_) => { toast(if checked { "Enabled" } else { "Disabled" }, "ok"); dom::spawn(render_tasks(p)); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        if let Some(sel) = dom::qs_in(&row, "[data-every]") {
            let (sel2, k2, p) = (sel.clone(), k.clone(), panel.clone());
            dom::on(&sel, "change", move |_| {
                let minutes = js_plus(&dom::value(&sel2));
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::patch(&format!("/admin/tasks/{k}"), json!({ "minutes": minutes })).await {
                        Ok(_) => { toast("Saved", "ok"); dom::spawn(render_tasks(p)); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        if let Some(run) = dom::qs_in(&row, "[data-run]") {
            let (run2, k2, p) = (run.clone(), k.clone(), panel.clone());
            dom::on(&run, "click", move |_| {
                dom::set_disabled(&run2, true);
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::post_empty(&format!("/admin/tasks/{k}/run")).await {
                        Ok(_) => { toast("Started", "ok"); dom::timeout(1500, move || dom::spawn(render_tasks(p))); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
    }
}
