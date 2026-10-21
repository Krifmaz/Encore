//! Port of web/js/pages/requests.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::pages::common::{self, bind_request_actions, clear_menu, request_row, Whose};
use crate::ui::{empty_state, esc, icon};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use web_sys::Element;

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let filter = { let s = ctx.q("status"); if s.is_empty() { "all".to_string() } else { s } };
        let filter = Rc::new(RefCell::new(filter));
        // whose requests show: members their own; admins everyone's, or one person's from the User list
        let whose = Rc::new(Cell::new(if admin { Whose::default() } else { Whose::mine() }));
        // the person picked in the User list (kept while they have no requests left, so the list doesn't jump)
        let picked: Rc<RefCell<Option<(i64, String)>>> = Rc::new(RefCell::new(None));
        let tabs: String = {
            let f = filter.borrow();
            [("all", "All"), ("active", "Active"), ("pending", "Awaiting approval"), ("completed", "Available"), ("failed", "Failed")]
                .iter()
                .map(|(k, l)| format!(r#"<button data-s="{k}" class="{}">{l}</button>"#, if *k == f.as_str() { "active" } else { "" }))
                .collect()
        };
        let controls = header_controls(admin, common::may_fix());
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Requests</h1><div class="small muted" id="quota"></div></div>
      <div class="row wrap" style="gap:8px">
        {controls}
      </div>
    </div>
    <div id="storage-banner"></div>
    <div class="tabs" id="r-tabs">
      {tabs}
    </div>
    <div class="card flat mt" id="r-list"><div class="empty"><span class="spinner"></span></div></div>"#));
        let list = dom::qs_in(&view, "#r-list").ok_or("#r-list")?;
        // imports paused because a folder can't be written: the banner over the list (R1)
        if let Some(slot) = dom::qs_in(&view, "#storage-banner") { common::storage_banner(&view, &slot, admin); }
        // the JS `load` closure: callers that never awaited it get a spawning `Rc<dyn Fn()>`
        let reload: Rc<dyn Fn()> = {
            let (view, list, filter, whose, picked) = (view.clone(), list.clone(), filter.clone(), whose.clone(), picked.clone());
            Rc::new(move || {
                let (view, list, f, w, picked) = (view.clone(), list.clone(), filter.borrow().clone(), whose.get(), picked.clone());
                dom::spawn(async move {
                    if let Err(e) = load(&list, &f, w, admin).await { dom::console_error(&e); }
                    if admin { paint_users(&view, &picked).await; }
                });
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
                // the User list: everyone's requests, or one person's (the buttons beside it take the same)
                let (whose, picked, reload) = (whose.clone(), picked.clone(), reload.clone());
                dom::on_sel(&view, "#r-user", "change", move |e| {
                    let Some(sel) = dom::target_el(e) else { return };
                    let user = dom::value(&sel).parse::<i64>().ok();
                    let name = dom::qs_in(&sel, "option:checked").map(|o| dom::text_of(&o)).unwrap_or_default();
                    *picked.borrow_mut() = user.map(|u| (u, name_of(&name)));
                    whose.set(Whose { mine: false, user });
                    reload();
                });
            }
            if let Some(btn) = dom::qs_in(&view, "#clear") {
                let (b, whose, reload) = (btn.clone(), whose.clone(), reload.clone());
                dom::on(&btn, "click", move |_| clear_menu(&b, reload.clone(), whose.get()));
            }
        }
        // Download all now (admins): every waiting request woken, the lines moved up (one person's with the User list)
        if let Some(btn) = dom::qs_in(&view, "#download-all-now") {
            let (whose, reload) = (whose.clone(), reload.clone());
            dom::on(&btn, "click", move |_| common::download_all_now(whose.get(), reload.clone()));
        }
        // Clear waiting: admins everyone's (or the person picked), members their own
        if let Some(btn) = dom::qs_in(&view, "#clear-waiting") {
            let (whose, reload) = (whose.clone(), reload.clone());
            dom::on(&btn, "click", move |_| common::clear_waiting(whose.get(), reload.clone()));
        }
        // members have the button too (their own failed requests): it was only wired for admins
        if let Some(btn) = dom::qs_in(&view, "#retry-failed") {
            let (whose, reload) = (whose.clone(), reload.clone());
            dom::on(&btn, "click", move |_| { let r = reload.clone(); common::retry_failed(whose.get(), r); });
        }
        let me = api::get("/auth/me").await.map_err(|e| e.message)?;
        if me.truthy("request_limit") && !matches!(me.s("role").as_str(), "admin" | "owner") {
            if let Some(q) = dom::qs_in(&view, "#quota") {
                dom::set_text(&q, &format!("{} of {} requests used this week", me.s("requests_this_week"), me.s("request_limit")));
            }
        }
        let f = filter.borrow().clone();
        load(&list, &f, whose.get(), admin).await?;
        if admin { paint_users(&view, &picked).await; }
        {
            let (list, reload) = (list.clone(), reload.clone());
            dom::interval(&view, 4000, move || { if dom::qs_in(&list, "details[open]").is_none() { reload(); } });
        }
        Ok(())
    })
}

/// The buttons over the list: admins get the User list, Retry failed, Download all now, Clear finished and Clear waiting;
/// members Clear waiting (their own), and Retry failed when they may fix their downloads (the others would only be
/// told "Ask an admin to retry").
fn header_controls(admin: bool, may_fix: bool) -> String {
    let waiting = format!(r#"<button class="btn sm" id="clear-waiting" title="Cancel every request that is still waiting">{}Clear waiting</button>"#, icon("x"));
    if admin {
        format!(r#"<label class="row" style="gap:6px"><span class="small muted">User</span><select class="input" id="r-user" style="width:auto;padding:5px 10px" aria-label="User"><option value="">{ALL_USERS}</option></select></label>
          <button class="btn sm" id="retry-failed" title="Queue every failed request again">{}Retry failed</button>
          <button class="btn sm" id="download-all-now" title="{}">{}Download all now</button>
          <button class="btn sm" id="clear">{}Clear finished</button>
          {waiting}"#, icon("refresh"), common::DOWNLOAD_ALL_NOW_TITLE, icon("download"), icon("trash"))
    } else if may_fix {
        format!(r#"<button class="btn sm" id="retry-failed" title="Queue every failed request again">{}Retry failed</button>
          {waiting}"#, icon("refresh"))
    } else {
        waiting
    }
}

/// The User list's first entry.
pub const ALL_USERS: &str = "All users";

/// "Caz (612)": a person in the User list with their requests.
pub fn user_label(name: &str, count: i64) -> String { format!("{name} ({count})") }

/// The name in a User list entry ("Caz (612)" → "Caz").
fn name_of(label: &str) -> String {
    match label.rfind(" (") { Some(i) if label.ends_with(')') => label[..i].to_string(), _ => label.to_string() }
}

/// The User list's entries (value, label): "All users", then each person who has requests (`GET /api/requests/users`)
/// with their count; the person `picked` stays in it with 0 once their requests are gone (the list doesn't jump to
/// everyone's).
pub fn user_options(users: &[Value], picked: Option<&(i64, String)>) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), ALL_USERS.to_string())];
    out.extend(users.iter().filter(|u| u.g("id").is_number()).map(|u| (u.i("id").to_string(), user_label(&u.s("username"), u.i("count")))));
    if let Some((id, name)) = picked {
        if !users.iter().any(|u| u.i("id") == *id) { out.push((id.to_string(), user_label(name, 0))); }
    }
    out
}

/// The User list drawn again with the counts now (not while it is open).
async fn paint_users(view: &Element, picked: &Rc<RefCell<Option<(i64, String)>>>) {
    let Some(sel) = dom::qs_in(view, "#r-user") else { return };
    let Ok(users) = api::get("/requests/users").await else { return };
    if dom::document().active_element().is_some_and(|a| a == sel) { return; }
    let p = picked.borrow().clone();
    let chosen = p.as_ref().map(|(id, _)| id.to_string()).unwrap_or_default();
    let html: String = user_options(&users.items(), p.as_ref()).iter()
        .map(|(v, l)| format!(r#"<option value="{v}"{}>{}</option>"#, if *v == chosen { " selected" } else { "" }, esc(l))).collect();
    if sel.inner_html() != html { dom::set_html(&sel, &html); }
}

/// The JS `load`: fetch the rows for the current filter and render them into `#r-list`.
async fn load(list: &Element, filter: &str, whose: Whose, admin: bool) -> Result<(), String> {
    let mut q = String::from("limit=300");
    if filter != "all" { q.push_str(&format!("&status={}", String::from(js_sys::encode_uri_component(filter)))); }
    q.push_str(&whose.query());
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

#[cfg(test)]
mod tests {
    use super::{header_controls, name_of, user_label, user_options};
    use serde_json::json;

    /// Admins pick whose requests show in a User list ("All users", then each person with their count) in place of
    /// Only mine; members have no list (they only ever see their own).
    #[test]
    fn the_user_list() {
        let admin = header_controls(true, true);
        assert!(admin.contains(r#"<select class="input" id="r-user""#) && admin.contains(r#"<option value="">All users</option>"#), "{admin}");
        assert!(!admin.contains("Only mine"), "{admin}");
        assert!(!header_controls(false, true).contains("r-user"));
        assert_eq!(user_label("Caz", 612), "Caz (612)");
        assert_eq!(name_of("Caz (612)"), "Caz");
        assert_eq!(name_of("Dr. (Who) (3)"), "Dr. (Who)");
        let users = vec![json!({ "id": 1, "username": "boss", "count": 1 }), json!({ "id": 7, "username": "Caz", "count": 612 })];
        let o = user_options(&users, None);
        assert_eq!(o, vec![("".into(), "All users".into()), ("1".into(), "boss (1)".into()), ("7".into(), "Caz (612)".into())]);
        // the person picked stays with 0 once their requests were cleared
        let o = user_options(&users[..1], Some(&(7, "Caz".into())));
        assert_eq!(o.last(), Some(&("7".to_string(), "Caz (0)".to_string())));
        assert_eq!(user_options(&users, Some(&(7, "Caz".into()))).len(), 3, "listed once");
    }

    /// Download all now is the admins' (the server answers "Admin only" to anyone else), beside Retry failed.
    #[test]
    fn download_all_now_is_for_admins() {
        let admin = header_controls(true, true);
        assert!(admin.contains(r#"id="download-all-now" title="Wake every waiting request and move requests up in their lines – never more at once than each source allows""#), "{admin}");
        assert!(admin.contains("Download all now</button>"), "{admin}");
        assert!(admin.find("retry-failed").unwrap() < admin.find("download-all-now").unwrap(), "beside Retry failed: {admin}");
        assert!(!header_controls(false, true).contains("download-all-now"));
        assert!(header_controls(false, true).contains("retry-failed"));
        assert!(!header_controls(false, false).contains("retry-failed"));
    }

    /// Clear waiting: next to Clear finished for admins, and there for every member (their own requests).
    #[test]
    fn clear_waiting_is_next_to_clear_finished_and_for_everyone() {
        let admin = header_controls(true, true);
        let (finished, waiting) = (admin.find(r#"id="clear""#).unwrap(), admin.find(r#"id="clear-waiting""#).unwrap());
        assert!(finished < waiting && admin[finished..waiting].matches("<button").count() == 1, "right after Clear finished: {admin}");
        for member in [header_controls(false, true), header_controls(false, false)] {
            assert!(member.contains("Clear waiting</button>") && !member.contains("Clear finished"), "{member}");
        }
        assert_eq!(crate::pages::common::waiting_question(612), "Cancel 612 waiting requests? Downloads and imports already running keep going.");
        assert_eq!(crate::pages::common::waiting_question(1), "Cancel 1 waiting request? Downloads and imports already running keep going.");
        assert_eq!(crate::pages::common::waiting_done(612), "Cancelled 612 waiting requests");
        assert_eq!(crate::pages::common::waiting_done(1), "Cancelled 1 waiting request");
        assert_eq!(crate::pages::common::waiting_done(0), "Nothing is waiting");
    }
}
