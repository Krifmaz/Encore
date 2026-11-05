//! Settings → Scheduled tasks: every background job, with on/off, interval, last result and Run now, and how long
//! finished downloads are kept. The page's settings card (Playlists & sync) comes from the schema under it.
//!
//! A Leptos view ([`Tasks`]) over one signal, the last answer of GET /api/admin/tasks; every change loads it again.

use crate::api;
use crate::dom;
use crate::json::{js_string, json, J};
use crate::ui::{icon, toast};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::Value;
use web_sys::Element;

/// The intervals to pick from: (minutes, words after "every").
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
/// When a task last ran, in words ("never", "just now", "5 min ago", …).
fn ago(t: f64) -> String {
    if t == 0.0 { return "never".into(); }
    let s = dom::now_s() - t;
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", js_round(s / 60.0)) } else if s < 129600.0 { format!("{} h ago", js_round(s / 3600.0)) } else { format!("{} days ago", js_round(s / 86400.0)) }
}
/// When a task runs next, in words ("any moment", "in 5 min", …); "" when not planned.
fn until(t: f64) -> String {
    if t == 0.0 { return String::new(); }
    let s = t - dom::now_s();
    if s <= 60.0 { "any moment".into() } else if s < 5400.0 { format!("in {} min", js_round(s / 60.0)) } else if s < 129600.0 { format!("in {} h", js_round(s / 3600.0)) } else { format!("in {} days", js_round(s / 86400.0)) }
}

/// A task's interval list: the usual ones and the task's own (when it is another), shortest first, as
/// (minutes, words).
fn intervals(minutes: f64) -> Vec<(f64, String)> {
    // [...new Set([...EVERY.map(e => e[0]), t.minutes])].sort((a, b) => a - b)
    let mut ms: Vec<f64> = EVERY.iter().map(|e| e.0 as f64).collect();
    if !ms.contains(&minutes) {
        ms.push(minutes);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ms.into_iter().map(|m| (m, EVERY.iter().find(|e| e.0 as f64 == m).map(|e| e.1.to_string()).unwrap_or_else(|| format!("{} minutes", js_num(m))))).collect()
}

/// "took 12 s" / "took 3 min" for a run that lasted `d` seconds.
fn took(d: f64) -> String {
    if d < 60.0 { format!("{} s", js_round(d)) } else { format!("{} min", js_round(d / 60.0)) }
}

/// `PATCH /admin/tasks/{key}` with `body`: `ok` as a toast and the tasks again, or the error.
fn patch_task(state: RwSignal<Option<Value>>, key: String, body: Value, ok: &'static str) {
    dom::spawn(async move {
        match api::patch(&format!("/admin/tasks/{key}"), body).await {
            Ok(_) => {
                toast(ok, "ok");
                reload(state).await;
            }
            Err(err) => toast(&err.message, "err"),
        }
    });
}

/// One task: its switch, name, what it does, its last and next run, its interval and Run now. A task an
/// environment variable sets is locked.
fn task_row(t: &Value, state: RwSignal<Option<Value>>) -> impl IntoView {
    let locked = t.truthy("locked");
    let enabled = t.truthy("enabled");
    let running = t.truthy("running");
    let minutes = t.f("minutes");
    let key = t.s("key");
    let every = intervals(minutes).into_iter().map(|(m, lab)| view! { <option value=js_num(m) selected=m == minutes>{lab}</option> }).collect_view();
    let took = if t.truthy("duration") { format!(" · took {}", took(t.f("duration"))) } else { String::new() };
    let run_note = running.then(|| view! { " · "<b style="color:var(--accent)">"running…"</b> });
    let next = if enabled && t.truthy("next_run") && !running { format!(" · next {}", until(t.f("next_run"))) } else { String::new() };
    // the last result and the next run, after the line break the old markup had there (one text with them, so the
    // line reads as before)
    let tail = if t.truthy("error") {
        view! { "\n           · "<span style="color:var(--err)">{t.s("error")}</span>{next} }.into_any()
    } else {
        let result = if t.truthy("result") && !running { format!(" · {}", t.s("result")) } else { String::new() };
        format!("\n          {result}{next}").into_any()
    };
    let k1 = key.clone();
    let on_switch = move |e: web_sys::Event| {
        let checked = dom::target_el(&e).is_some_and(|t| dom::checked(&t));
        patch_task(state, k1.clone(), json!({ "enabled": checked }), if checked { "Enabled" } else { "Disabled" });
    };
    let k2 = key.clone();
    let on_every = move |e: web_sys::Event| {
        let minutes = dom::target_el(&e).map(|t| js_plus(&dom::value(&t))).unwrap_or(Value::Null);
        patch_task(state, k2.clone(), json!({ "minutes": minutes }), "Saved");
    };
    let k3 = key.clone();
    let on_run = move |e: web_sys::MouseEvent| {
        if let Some(b) = dom::target_el(&e).and_then(|t| dom::closest_of(&t, "button")) {
            dom::set_disabled(&b, true);
        }
        let k = k3.clone();
        dom::spawn(async move {
            match api::post_empty(&format!("/admin/tasks/{k}/run")).await {
                Ok(_) => {
                    toast("Started", "ok");
                    dom::timeout(1500, move || dom::spawn(reload(state)));
                }
                Err(err) => toast(&err.message, "err"),
            }
        });
    };
    view! {
        <div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-k=key>
            <label class="switch" style="margin-top:2px" title=if locked { "Set by an environment variable" } else { "" }><input type="checkbox" data-on checked=enabled disabled=locked on:change=on_switch/><i></i></label>
            <div class="grow" style="min-width:220px"><div style="font-weight:650">{t.s("name")}{locked.then(|| view! { " "<span title="Set by an environment variable">"🔒"</span> })}</div>
                <div class="small muted">{t.s("desc")}</div>
                <div class="small faint" style="margin-top:4px">{format!("Last run {}{took}", ago(t.f("last_run")))}{run_note}{tail}</div></div>
            <div class="row" style="gap:8px"><span class="small muted">"every"</span>
                <select class="input" data-every style="width:auto" disabled=locked on:change=on_every>{every}</select>
                <button class="btn sm" data-run disabled=running on:click={on_run} {..html(format!("{}Run now", icon("play")))}>{""}</button></div>
        </div>
    }
}

/// The tasks fetched again into `state` (a failure only reaches the console, as before).
async fn reload(state: RwSignal<Option<Value>>) {
    match api::get("/admin/tasks").await {
        Ok(d) => {
            state.try_set(Some(d));
        }
        Err(e) => dom::console_error(&e.message),
    }
}

/// Settings → Scheduled tasks into `panel` (nothing when the tasks can't be loaded; the error goes to the console).
pub async fn render_tasks(panel: Element) {
    let d = match api::get("/admin/tasks").await {
        Ok(d) => d,
        Err(e) => {
            dom::console_error(&e.message);
            return;
        }
    };
    dom::set_html(&panel, "");
    // the signal lives as long as the page: the owner is dropped with it
    let owner = Owner::new();
    let state = owner.with(|| RwSignal::new(Some(d)));
    if let Some(m) = owner.with(|| kit_leptos::rt::mount(&panel, move || view! { <Tasks state/> })) {
        dom::keep(m);
    }
    dom::keep(owner);
}

/// The Scheduled tasks card: Refresh, every task, and how long finished downloads are kept.
#[component]
fn Tasks(
    /// The last answer of GET /api/admin/tasks.
    state: RwSignal<Option<Value>>,
) -> impl IntoView {
    let on_days = move |e: web_sys::Event| {
        let days = dom::target_el(&e).map(|t| js_plus(&dom::value(&t))).unwrap_or(Value::Null);
        dom::spawn(async move {
            match api::put("/admin/settings", json!({ "task_cleanup_days": days })).await {
                Ok(_) => toast("Saved", "ok"),
                Err(e) => dom::console_error(&e.message),
            }
        });
    };
    move || {
        let d = state.get()?;
        let rows = d.a("tasks").iter().map(|t| task_row(t, state)).collect_view();
        Some(view! {
            <div class="card"><div class="row between" style="margin-bottom:8px"><h2>"Scheduled tasks"</h2><button class="btn sm" id="t-refresh" on:click=move |_| dom::spawn(reload(state)) {..html(format!("{}Refresh", icon("refresh")))}>{""}</button></div>
                <p class="muted small" style="margin:0 0 14px">"Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now."</p>
                <div class="list">{rows}</div>
                <div class="row mt" style="gap:8px"><span class="small muted">"Clean up finished downloads older than"</span>
                    <input class="input" type="number" min="1" id="t-days" value=js_string(d.g("cleanup_days")) style="width:90px" on:change=on_days/><span class="small muted">"days"</span></div></div>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task's own interval joins the list in its place; the usual ones keep their words.
    #[test]
    fn a_tasks_own_interval_joins_the_list() {
        let l = intervals(45.0);
        assert_eq!(l.len(), EVERY.len() + 1);
        assert_eq!(l[2], (45.0, "45 minutes".to_string()));
        assert_eq!(intervals(60.0).len(), EVERY.len());
        assert_eq!(took(12.4), "12 s");
        assert_eq!(took(150.0), "3 min");
    }
}
