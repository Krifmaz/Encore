//! Port of web/js/pages/requests.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::pages::common::{self, bind_request_actions, clear_menu, request_row};
use crate::ui::{empty_state, icon};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use web_sys::Element;

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let filter = { let s = ctx.q("status"); if s.is_empty() { "all".to_string() } else { s } };
        let filter = Rc::new(RefCell::new(filter));
        let mine = Rc::new(Cell::new(!admin));
        let tabs: String = {
            let f = filter.borrow();
            [("all", "All"), ("active", "Active"), ("pending", "Awaiting approval"), ("completed", "Available"), ("failed", "Failed")]
                .iter()
                .map(|(k, l)| format!(r#"<button data-s="{k}" class="{}">{l}</button>"#, if *k == f.as_str() { "active" } else { "" }))
                .collect()
        };
        let controls = if admin {
            format!(r#"<label class="switch"><input type="checkbox" id="mine"><i></i>Only mine</label>
          <button class="btn sm" id="retry-failed" title="Queue every failed request again">{}Retry failed</button>
          <button class="btn sm" id="clear">{}Clear finished</button>"#, icon("refresh"), icon("trash"))
        } else { format!(r#"<button class="btn sm" id="retry-failed" title="Queue every failed request again">{}Retry failed</button>"#, icon("refresh")) };
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Requests</h1><div class="small muted" id="quota"></div></div>
      <div class="row">
        {controls}
      </div>
    </div>
    <div class="tabs" id="r-tabs">
      {tabs}
    </div>
    <div class="card flat mt" id="r-list"><div class="empty"><span class="spinner"></span></div></div>"#));
        let list = dom::qs_in(&view, "#r-list").ok_or("#r-list")?;
        // the JS `load` closure: callers that never awaited it get a spawning `Rc<dyn Fn()>`
        let reload: Rc<dyn Fn()> = {
            let (list, filter, mine) = (list.clone(), filter.clone(), mine.clone());
            Rc::new(move || {
                let (list, f, m) = (list.clone(), filter.borrow().clone(), mine.get());
                dom::spawn(async move { if let Err(e) = load(&list, &f, m, admin).await { dom::console_error(&e); } });
            })
        };
        bind_request_actions(&list, reload.clone());
        {
            let (v, filter, reload) = (view.clone(), filter.clone(), reload.clone());
            dom::on_sel(&view, "#r-tabs", "click", move |e| {
                let Some(t) = dom::closest(e, "[data-s]") else { return };
                *filter.borrow_mut() = dom::data(&t, "s");
                for b in dom::qsa_in(&v, "#r-tabs button") { dom::toggle_class(&b, "active", b == t); }
                reload();
            });
        }
        if admin {
            {
                let (mine, reload) = (mine.clone(), reload.clone());
                dom::on_sel(&view, "#mine", "change", move |e| {
                    mine.set(dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false));
                    reload();
                });
            }
            if let Some(btn) = dom::qs_in(&view, "#clear") {
                let (b, mine, reload) = (btn.clone(), mine.clone(), reload.clone());
                dom::on(&btn, "click", move |_| clear_menu(&b, reload.clone(), mine.get()));
            }
            if let Some(btn) = dom::qs_in(&view, "#retry-failed") {
                let (mine, reload) = (mine.clone(), reload.clone());
                dom::on(&btn, "click", move |_| { let r = reload.clone(); common::retry_failed(mine.get(), r); });
            }
        }
        let me = api::get("/auth/me").await.map_err(|e| e.message)?;
        if me.truthy("request_limit") && !matches!(me.s("role").as_str(), "admin" | "owner") {
            if let Some(q) = dom::qs_in(&view, "#quota") {
                dom::set_text(&q, &format!("{} of {} requests used this week", me.s("requests_this_week"), me.s("request_limit")));
            }
        }
        let f = filter.borrow().clone();
        load(&list, &f, mine.get(), admin).await?;
        {
            let (list, reload) = (list.clone(), reload.clone());
            dom::interval(&view, 4000, move || { if dom::qs_in(&list, "details[open]").is_none() { reload(); } });
        }
        Ok(())
    })
}

/// The JS `load`: fetch the rows for the current filter and render them into `#r-list`.
async fn load(list: &Element, filter: &str, mine: bool, admin: bool) -> Result<(), String> {
    let mut q = String::from("limit=300");
    if filter != "all" { q.push_str(&format!("&status={}", String::from(js_sys::encode_uri_component(filter)))); }
    if mine { q.push_str("&mine=true"); }
    let rows = api::get(&format!("/requests?{q}")).await.map_err(|e| e.message)?.items();
    let top: Vec<&Value> = rows.iter().filter(|r| !r.truthy("parent_id") || !rows.iter().any(|p| p.g("id") == r.g("parent_id"))).collect();
    let html = if !top.is_empty() {
        format!(r#"<div class="list">{}</div>"#, top.iter().map(|r| {
            let kids: Vec<&Value> = rows.iter().filter(|c| c.g("parent_id") == r.g("id")).collect();
            let mut s = request_row(r, admin);
            if !kids.is_empty() {
                s.push_str(&format!(r#"<details style="padding:0 16px 10px 78px"><summary class="small muted" style="cursor:pointer">{} child request(s) · {} available</summary><div class="list">{}</div></details>"#,
                    kids.len(),
                    kids.iter().filter(|k| k.s("status") == "completed").count(),
                    kids.iter().map(|k| request_row(k, admin)).collect::<String>()));
            }
            s
        }).collect::<String>())
    } else {
        empty_state(&icon("inbox"), "Nothing here", r##"Head to <a href="#/search" style="color:var(--accent)">Discover</a> to request music."##)
    };
    dom::set_html(list, &html);
    Ok(())
}
