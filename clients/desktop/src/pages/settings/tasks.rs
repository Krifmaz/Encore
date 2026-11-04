//! Settings → Scheduled tasks. Behaviour reference: web-rs/src/pages/settings.rs `render_tasks`:
//! every background job with on/off, how often it runs, its last result and Run now, plus the
//! clean-up age of finished downloads.

use super::{help, input, js_num, js_plus, patch, select, switch_maybe, Opt};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::{self, btn_icon, t, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};

const EVERY: &[(i64, &str)] = &[
    (15, "15 minutes"),
    (30, "30 minutes"),
    (60, "hour"),
    (180, "3 hours"),
    (360, "6 hours"),
    (720, "12 hours"),
    (1440, "day"),
    (10080, "week"),
];

#[derive(Debug, Default)]
pub struct State {
    data: Option<Value>,
    days: String,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Refresh,
    Enabled(String, bool),
    Every(String, String),
    Run(String),
    Patched(&'static str, Result<Value, ApiError>),
    Ran(Result<Value, ApiError>),
    DaysInput(String),
    DaysCommit,
    DaysSaved(Result<Value, ApiError>),
}

fn load(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/tasks").await }, Message::Loaded)
}

/// Reload a moment later (a task just started).
fn load_soon(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            api.get("/admin/tasks").await
        },
        Message::Loaded,
    )
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.error.clear();
    load(ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(d)) => {
            state.days = super::vs(&d["cleanup_days"]);
            state.data = Some(d);
        }
        Message::Loaded(Err(e)) => state.error = e.message,
        Message::Refresh => return load(ctx),
        Message::Enabled(k, on) => {
            let api = ctx.api.clone();
            return Task::perform(async move { patch(&api, &format!("/admin/tasks/{k}"), json!({ "enabled": on })).await }, move |r| {
                Message::Patched(if on { "Enabled" } else { "Disabled" }, r)
            });
        }
        Message::Every(k, minutes) => {
            let api = ctx.api.clone();
            let m = js_plus(&minutes);
            return Task::perform(async move { patch(&api, &format!("/admin/tasks/{k}"), json!({ "minutes": m })).await }, |r| Message::Patched("Saved", r));
        }
        Message::Run(k) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/admin/tasks/{k}/run")).await }, Message::Ran);
        }
        Message::Patched(word, Ok(_)) => {
            ctx.toast_ok(word);
            return load(ctx);
        }
        Message::Patched(_, Err(e)) => ctx.toast_err(e.message),
        Message::Ran(Ok(_)) => {
            ctx.toast_ok("Started");
            return load_soon(ctx);
        }
        Message::Ran(Err(e)) => ctx.toast_err(e.message),
        Message::DaysInput(s) => state.days = s,
        Message::DaysCommit => {
            let api = ctx.api.clone();
            let days = js_plus(&state.days);
            return Task::perform(async move { api.put("/admin/settings", json!({ "task_cleanup_days": days })).await }, Message::DaysSaved);
        }
        Message::DaysSaved(Ok(_)) => ctx.toast_ok("Saved"),
        Message::DaysSaved(Err(e)) => ctx.toast_err(e.message),
    }
    Task::none()
}

/// The "every" options of a task: the table plus its own interval when that is not in it.
pub fn every_options(minutes: f64) -> Vec<Opt> {
    let mut ms: Vec<f64> = EVERY.iter().map(|e| e.0 as f64).collect();
    if !ms.contains(&minutes) {
        ms.push(minutes);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ms.iter()
        .map(|m| {
            let label = EVERY.iter().find(|e| e.0 as f64 == *m).map(|e| e.1.to_string()).unwrap_or_else(|| format!("{} minutes", js_num(*m)));
            Opt::new(js_num(*m), label)
        })
        .collect()
}

/// "Last run 3 h ago · took 12 s · running… · <result> · next in 2 h".
pub fn last_line(t: &Value, now: f64) -> String {
    let running = fmt::truthy(t, "running");
    let enabled = fmt::truthy(t, "enabled");
    let mut s = format!("Last run {}", super::ago_from(now, fmt::f(t, "last_run")));
    if fmt::truthy(t, "duration") {
        let d = fmt::f(t, "duration");
        s.push_str(&format!(" · took {}", if d < 60.0 { format!("{} s", super::js_round(d)) } else { format!("{} min", super::js_round(d / 60.0)) }));
    }
    if running {
        s.push_str(" · running…");
    }
    if fmt::truthy(t, "error") {
        s.push_str(&format!(" · {}", fmt::s(t, "error")));
    } else if fmt::truthy(t, "result") && !running {
        s.push_str(&format!(" · {}", fmt::s(t, "result")));
    }
    if enabled && fmt::truthy(t, "next_run") && !running {
        s.push_str(&format!(" · next {}", super::until_from(now, fmt::f(t, "next_run"))));
    }
    s
}

fn task_row<'a>(task: &'a Value) -> El<'a, Message> {
    let p = theme::look();
    let key = fmt::s(task, "key");
    let locked = fmt::truthy(task, "locked");
    let enabled = fmt::truthy(task, "enabled");
    let running = fmt::truthy(task, "running");
    let minutes = fmt::f(task, "minutes");
    let (k1, k2, k3) = (key.clone(), key.clone(), key);
    let mut name = row![t(fmt::s(task, "name")).size(14).font(theme::semibold())].spacing(6).align_y(Alignment::Center);
    if locked {
        name = name.push(t("🔒").size(12));
    }
    let line = last_line(task, super::now_s());
    let has_error = fmt::truthy(task, "error");
    let left = column![
        name,
        help(fmt::s(task, "desc")),
        widgets::faint(line).size(12).color(if has_error { p.err } else if running { p.accent } else { p.faint }),
    ]
    .spacing(3)
    .width(Length::Fill);
    let right = row![
        help("every"),
        if locked {
            help(every_options(minutes).into_iter().find(|o| o.key == js_num(minutes)).map(|o| o.label).unwrap_or_default()).into()
        } else {
            select(every_options(minutes), &js_num(minutes), move |m| Message::Every(k2.clone(), m))
        },
        btn_icon(Icon::Play, "Run now", Btn::Default, (!running).then_some(Message::Run(k3))),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let r = row![
        switch_maybe(enabled, "", (!locked).then_some(move |on| Message::Enabled(k1.clone(), on))),
        left,
        right,
    ]
    .spacing(12)
    .align_y(Alignment::Start);
    column![container(r).padding([10, 14]), widgets::divider()].into()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(d) = &state.data else {
        return widgets::card(widgets::skeleton_rows(4)).width(Length::Fill).into();
    };
    let tasks: &'a [Value] = d["tasks"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let mut list = column![];
    for task in tasks {
        list = list.push(task_row(task));
    }
    widgets::card(
        column![
            super::between(
                widgets::h2("Scheduled tasks").into(),
                btn_icon(Icon::Refresh, "Refresh", Btn::Default, Some(Message::Refresh)).into(),
            ),
            help("Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now."),
            widgets::card_flat(list).width(Length::Fill),
            row![
                help("Clean up finished downloads older than"),
                input("", &state.days, Message::DaysInput).on_submit(Message::DaysCommit).width(80),
                help("days"),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_like_the_web() {
        let opts = every_options(45.0);
        assert_eq!(opts.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(), ["15", "30", "45", "60", "180", "360", "720", "1440", "10080"]);
        assert_eq!(opts[2].label, "45 minutes");
        assert_eq!(opts[3].label, "hour");
        let now = 100_000.0;
        let t = json!({ "enabled": true, "last_run": now - 7200.0, "duration": 12.4, "result": "3 new", "next_run": now + 7200.0 });
        assert_eq!(last_line(&t, now), "Last run 2 h ago · took 12 s · 3 new · next in 2 h");
        let soon = json!({ "enabled": true, "last_run": now - 60.0, "next_run": now + 3600.0 });
        assert_eq!(last_line(&soon, now), "Last run just now · next in 60 min");
        let r = json!({ "enabled": true, "running": true, "last_run": 0, "result": "x" });
        assert_eq!(last_line(&r, now), "Last run never · running…");
        let e = json!({ "enabled": false, "last_run": now - 10.0, "duration": 125.0, "error": "failed" });
        assert_eq!(last_line(&e, now), "Last run just now · took 2 min · failed");
    }
}
