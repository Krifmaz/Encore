//! The download activity list: Soulseek transfers, torrent/Usenet downloads and the activity log, reloaded every 5
//! seconds while the page is on screen (it is part of the Downloader page for admins).
//!
//! A Leptos view ([`Activity`]) over two signals: the last answers of GET /api/system/downloads and
//! GET /api/system/activity, and the log's level filter. The page's first load fails the page like before; the
//! reloads after it keep the last answer when they fail.

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{empty_state, fmt, icon, toast};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use web_sys::Element;

/// Both lists at once (the downloads and the log).
async fn fetch() -> Result<(Value, Value), String> {
    futures::future::try_join(api::get("/system/downloads"), api::get("/system/activity")).await.map_err(|e| e.message)
}

/// Loads both lists into `answer`; a failure leaves the last answer (the old page let it go unseen too).
fn reload(answer: RwSignal<Option<(Value, Value)>>) {
    dom::spawn(async move {
        if let Ok(a) = fetch().await {
            answer.set(Some(a));
        }
    });
}

/// The Activity page: its heading, then the view. The first load's failure is the page's error.
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let answer = owner.with(|| RwSignal::new(None));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Activity answer/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        answer.set(Some(fetch().await?));
        dom::interval(&view, 5000, move || reload(answer));
        Ok(())
    })
}

/// The three cards over the answers (`None`: still loading).
#[component]
pub fn Activity(
    /// The last answers of /system/downloads and /system/activity.
    answer: RwSignal<Option<(Value, Value)>>,
) -> impl IntoView {
    let level = RwSignal::new(String::new());
    let clear = move |_| {
        dom::spawn(async move {
            match api::post_empty("/system/downloads/slskd/clear").await {
                Ok(_) => reload(answer),
                Err(e) => toast(&e.message, "err"),
            }
        });
    };
    let on_level = move |e: web_sys::Event| {
        level.set(dom::target_el(&e).map(|t| dom::value(&t)).unwrap_or_default());
        reload(answer);
    };
    let slskd = move || match answer.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some((d, _)) => slskd_card(&d).into_any(),
    };
    let torrents = move || answer.get().map(|(d, _)| torrents_card(&d));
    let log = move || answer.get().map(|(_, logs)| log_card(&logs, &level.get()));
    view! {
        <h1 style="margin-bottom:18px">"Activity"</h1>
        <div class="section-head"><h2>"Soulseek transfers"</h2><button class="btn sm" id="clear-slskd" on:click={clear} {..html(format!("{}Clear completed", icon("trash")))}>{""}</button></div>
        <div class="card flat" id="slskd">{slskd}</div>
        <div class="section-head"><h2>"Torrents & Usenet"</h2></div>
        <div class="card flat" id="torrents">{torrents}</div>
        <div class="section-head"><h2>"Log"</h2>
            <select class="input" id="lvl" style="width:auto" on:change=on_level><option value="">"All levels"</option><option value="info">"Info"</option><option value="warning">"Warnings"</option><option value="error">"Errors"</option></select></div>
        <div class="card flat" id="log">{log}</div>
    }
}

/// The Soulseek card: slskd's error, its transfers (the first 200), or that there are none.
fn slskd_card(d: &Value) -> AnyView {
    if d.g("errors").truthy("slskd") {
        return view! { <div class="error-box" style="margin:14px">{d.g("errors").s("slskd")}</div> }.into_any();
    }
    let files = d.a("slskd");
    if files.is_empty() {
        return raw(empty_state(&icon("download"), "No Soulseek transfers", "slskd may not be configured.")).into_any();
    }
    let rows = files.iter().take(200).map(slskd_row).collect_view();
    view! {
        <div class="table-wrap"><table class="table"><thead><tr><th>"File"</th><th class="hide-sm">"User"</th><th>"State"</th><th style="width:180px">"Progress"</th></tr></thead><tbody>
            {rows}
        </tbody></table></div>
    }
    .into_any()
}

/// The badge class of a transfer's state.
fn state_class(state: &str) -> &'static str {
    if state.contains("Succeeded") {
        "ok"
    } else if ["Errored", "Rejected", "TimedOut", "Failed", "Aborted"].iter().any(|k| state.contains(k)) {
        "err"
    } else {
        "info"
    }
}

/// One row of the Soulseek table.
fn slskd_row(f: &Value) -> impl IntoView {
    let state = f.s("state");
    let speed = if f.truthy("speed") { format!(" · {}/s", fmt::size(f.f("speed"))) } else { String::new() };
    // slskd's reason for a failed transfer, whole on a click, with Copy
    let error = f.truthy("error").then(|| {
        let whole = format!("{}\n{}\n{}", f.s("username"), f.s("filename"), f.s("error"));
        view! { <div class="small" style="color:var(--err);max-width:420px;overflow-wrap:anywhere" data-details=whole data-details-title="Transfer error">{f.s("error")}</div> }
    });
    view! {
        <tr><td><div class="ellipsis" style="max-width:420px;font-weight:600">{f.s("filename")}</div><div class="small faint ellipsis" style="max-width:420px">{format!("{} · {}", f.s("directory"), fmt::size(f.f("size")))}</div>{error}</td>
            <td class="hide-sm muted">{f.s("username")}</td>
            <td><span class=format!("badge {}", state_class(&state))>{state.clone()}</span></td>
            <td><div class="progress"><i style=format!("width:{}%", f.s("progress"))></i></div><div class="small faint">{format!("{}%{speed}", f.s("progress"))}</div></td></tr>
    }
}

/// The torrents and Usenet card: what is downloading, or that nothing is.
fn torrents_card(d: &Value) -> AnyView {
    let tor = d.a("torrents");
    if tor.is_empty() {
        return raw(empty_state(&icon("download"), "Nothing downloading from torrents or Usenet", "")).into_any();
    }
    let rows = tor
        .iter()
        .map(|q| {
            let name = format!("{} – {}", q.s("artist"), q.s("title"));
            // the whole message on a click, with Copy
            let message = q.truthy("message").then(|| view! { <div class="small muted" data-details=q.s("message") data-details-title=name.clone()>{q.s("message")}</div> });
            view! {
                <div class="list-item"><div class="grow"><b>{name.clone()}</b>
                    {message}
                    <div class="progress" style="margin-top:6px;max-width:420px"><i style=format!("width:{}%", q.f("progress").round() as i64)></i></div></div></div>
            }
        })
        .collect_view();
    view! { <div class="list">{rows}</div> }.into_any()
}

/// The log card: the entries of the chosen level (the first 200), or that there are none.
fn log_card(logs: &Value, level: &str) -> AnyView {
    let rows: Vec<Value> = logs.items().into_iter().filter(|l| level.is_empty() || l.g("level").as_str() == Some(level)).collect();
    if rows.is_empty() {
        return view! { <div class="empty muted">"No log entries"</div> }.into_any();
    }
    let rows = rows.iter().take(200).map(log_row).collect_view();
    view! { <div class="list">{rows}</div> }.into_any()
}

/// One entry of the log: its level, the message (a click opens it with its level and time, to read and copy) and
/// how long ago.
fn log_row(l: &Value) -> impl IntoView {
    let level = l.s("level");
    let cls = if level == "error" { "err" } else if level == "warning" { "warn" } else { "" };
    let when = fmt::date(l.f("ts"));
    view! {
        <div class="list-item" style="padding:8px 16px">
            <span class=format!("badge {cls}") style="width:68px;justify-content:center">{level.clone()}</span>
            <span class="grow small log-text" data-details=log_text(l, &when) data-details-title="Log entry">{l.s("message")}</span><span class="small faint" title=when.clone()>{fmt::ago(l.f("ts"))}</span></div>
    }
}

/// One log line as text: "warning · 19/10/2026, 14:10:00" and the message under it.
pub fn log_text(l: &Value, when: &str) -> String {
    format!("{} · {when}\n{}", l.s("level"), l.s("message"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transfer_states_get_their_colour() {
        assert_eq!(state_class("Completed, Succeeded"), "ok");
        assert_eq!(state_class("Completed, TimedOut"), "err");
        assert_eq!(state_class("InProgress"), "info");
    }

    #[test]
    fn a_log_line_reads_with_its_level_and_time() {
        assert_eq!(log_text(&json!({ "level": "warning", "message": "disk slow" }), "t"), "warning · t\ndisk slow");
    }
}
