//! The Requests page: the reader's requests (admins: everyone's, or one person's from the User list) by status.
//!
//! The tabs carry how many requests each holds, and a line of badges under the title says how many downloads
//! are still to go, await approval or failed (`GET /api/requests/counts`, whose requests the list shows),
//! refreshed with the list every 4 s (not while a request's children are unfolded).
//!
//! A Leptos view ([`Requests`]) over the page's signals. The request rows are shared with other pages and still
//! drawn as HTML by `common` (with their actions).
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::pages::common::{self, bind_request_actions, clear_menu, request_row, Whose};
use crate::ui::{empty_state, icon};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;

/// The page's state.
#[derive(Clone, Copy)]
struct Page {
    /// Whether the reader is an admin.
    admin: bool,
    /// The tab: `all` or a request status.
    filter: RwSignal<String>,
    /// Whose requests show: members their own; admins everyone's, or one person's from the User list.
    whose: RwSignal<Whose>,
    /// The person picked in the User list (kept while they have no requests left, so the list doesn't jump).
    picked: StoredValue<Option<(i64, String)>>,
    /// The counts of `GET /api/requests/counts` (`None` until they are in).
    counts: RwSignal<Option<Value>>,
    /// The User list's entries (value, label).
    users: RwSignal<Vec<(String, String)>>,
    /// "3 of 10 requests used this week" for a member with a limit.
    quota: RwSignal<String>,
    /// The list's rows (`None`: still loading).
    rows: RwSignal<Option<Vec<Value>>>,
}

impl Page {
    /// The list, the counts and (admins) the User list, fetched again.
    async fn refresh(self) {
        if let Err(e) = load(self).await {
            dom::console_error(&e);
        }
        paint_counts(self).await;
        if self.admin {
            paint_users(self).await;
        }
    }

    /// [`Page::refresh`] as the callback the shared request actions take (not awaited).
    fn reload(self) -> Rc<dyn Fn()> {
        Rc::new(move || dom::spawn(self.refresh()))
    }
}

/// The Requests page: the view at once, then the reader's quota and the first list (their failure is the page's
/// error), then the counts; the list is fetched again every 4 s.
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let filter = {
            let s = ctx.q("status");
            if s.is_empty() { "all".to_string() } else { s }
        };
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let page = owner.with(|| Page {
            admin,
            filter: RwSignal::new(filter),
            whose: RwSignal::new(if admin { Whose::default() } else { Whose::mine() }),
            picked: StoredValue::new(None),
            counts: RwSignal::new(None),
            users: RwSignal::new(vec![(String::new(), ALL_USERS.to_string())]),
            quota: RwSignal::new(String::new()),
            rows: RwSignal::new(None),
        });
        let may_fix = common::may_fix();
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Requests page may_fix/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        let list = dom::qs_in(&view, "#r-list").ok_or("#r-list")?;
        // imports paused because a folder can't be written: the banner over the list (R1)
        if let Some(slot) = dom::qs_in(&view, "#storage-banner") {
            common::storage_banner(&view, &slot, admin);
        }
        bind_request_actions(&list, page.reload());
        let me = api::get("/auth/me").await.map_err(|e| e.message)?;
        if me.truthy("request_limit") && !matches!(me.s("role").as_str(), "admin" | "owner") {
            page.quota.set(format!("{} of {} requests used this week", me.s("requests_this_week"), me.s("request_limit")));
        }
        load(page).await?;
        paint_counts(page).await;
        if admin {
            paint_users(page).await;
        }
        let reload = page.reload();
        dom::interval(&view, 4000, move || {
            if dom::qs_in(&list, "details[open]").is_none() {
                reload();
            }
        });
        Ok(())
    })
}

/// The page: the title with the counts and the reader's quota, the buttons over the list, the storage banner,
/// the status tabs and the list.
#[component]
fn Requests(
    /// The page's state.
    page: Page,
    /// Whether a member may fix their own downloads (then they have Retry failed).
    may_fix: bool,
) -> impl IntoView {
    let tabs = TABS
        .iter()
        .map(|&(k, l)| {
            let pick = move |_| {
                page.filter.set(k.to_string());
                dom::spawn(page.refresh());
            };
            let count = move || page.counts.with(|c| c.as_ref().map(|c| c.i(k))).map(|n| view! { " "<span class="tab-count">{n}</span> });
            view! { <button data-s=k class=move || if page.filter.get() == k { "active" } else { "" } on:click=pick>{l}{count}</button> }
        })
        .collect_view();
    let badges = move || page.counts.get().map(|c| counts_badges(&c));
    let list = move || {
        page.rows.get().map(|rows| {
            if rows.is_empty() {
                return raw(empty_state(&icon("inbox"), "Nothing here", r##"Head to <a href="#/search" style="color:var(--accent)">Discover</a> to request music."##));
            }
            raw(list_html(&rows, page.admin))
        })
    };
    let controls = controls(page.admin, may_fix).into_iter().map(|c| control_view(c, page)).collect_view();
    view! {
        <div class="row wrap between" style="margin-bottom:18px">
            <div><h1>"Requests"</h1><div class="row wrap" id="r-counts" style="gap:6px;margin-top:4px">{badges}</div><div class="small muted" id="quota">{move || page.quota.get()}</div></div>
            <div class="row wrap" style="gap:8px">
                {controls}
            </div>
        </div>
        <div id="storage-banner"></div>
        <div class="tabs" id="r-tabs">
            {tabs}
        </div>
        <div class="card flat mt" id="r-list">{move || list().unwrap_or_else(|| raw(r#"<div class="empty"><span class="spinner"></span></div>"#))}</div>
    }
}

/// The status tabs: `(key, label)`; the key is the list's `status` and the field of `GET /api/requests/counts`
/// that counts the tab.
const TABS: [(&str, &str); 5] = [("all", "All"), ("active", "Active"), ("pending", "Awaiting approval"), ("completed", "Available"), ("failed", "Failed")];

/// What `GET /api/requests/counts` puts in the badges under the title: (field, badge class, words).
const BADGES: [(&str, &str, &str); 3] = [("remaining", "info", "remaining"), ("pending", "warn", "awaiting approval"), ("failed", "err", "failed")];

/// The badges under the title: the downloads still to go, the requests awaiting approval and the failed ones
/// (field, badge class, text); a group with none is left out, and nothing at all when nothing is left to do.
pub fn badges(c: &Value) -> Vec<(&'static str, &'static str, String)> {
    BADGES.iter().filter(|(k, _, _)| c.i(k) > 0).map(|&(k, class, text)| (k, class, format!("{} {text}", c.i(k)))).collect()
}

/// The badges of [`badges`] as views.
fn counts_badges(c: &Value) -> impl IntoView {
    badges(c).into_iter().map(|(k, class, text)| view! { <span class=format!("badge {class}") data-count=k>{text}</span> }).collect_view()
}

/// The counts fetched for whose requests the list shows, into the tabs and the badges under the title (only
/// when they changed). A failed fetch leaves what was drawn last (the list still works without them).
async fn paint_counts(page: Page) {
    let q = page.whose.get_untracked().query();
    let Ok(c) = api::get(&format!("/requests/counts?{}", q.trim_start_matches('&'))).await else { return };
    if page.counts.try_with_untracked(|old| old.as_ref() != Some(&c)) == Some(true) {
        page.counts.set(Some(c));
    }
}

/// A button over the list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Control {
    /// The User list: everyone's requests, or one person's.
    User,
    /// Queue every failed request again.
    RetryFailed,
    /// Wake every waiting request.
    DownloadAllNow,
    /// The Clear finished menu.
    ClearFinished,
    /// Cancel every request that is still waiting.
    ClearWaiting,
}

/// The buttons over the list: admins get the User list, Retry failed, Download all now, Clear finished and Clear
/// waiting; members Clear waiting (their own), and Retry failed when they may fix their downloads (the others
/// would only be told "Ask an admin to retry").
fn controls(admin: bool, may_fix: bool) -> Vec<Control> {
    use Control::*;
    if admin {
        vec![User, RetryFailed, DownloadAllNow, ClearFinished, ClearWaiting]
    } else if may_fix {
        vec![RetryFailed, ClearWaiting]
    } else {
        vec![ClearWaiting]
    }
}

/// One button over the list; each takes whose requests the list shows.
fn control_view(c: Control, page: Page) -> AnyView {
    let whose = page.whose;
    match c {
        Control::User => {
            let on_change = move |e: web_sys::Event| {
                let Some(sel) = dom::target_el(&e) else { return };
                let user = dom::value(&sel).parse::<i64>().ok();
                let name = dom::qs_in(&sel, "option:checked").map(|o| dom::text_of(&o)).unwrap_or_default();
                page.picked.set_value(user.map(|u| (u, name_of(&name))));
                whose.set(Whose { mine: false, user });
                dom::spawn(page.refresh());
            };
            let options = move || {
                let chosen = page.picked.get_value().map(|(id, _)| id.to_string()).unwrap_or_default();
                page.users.get().into_iter().map(|(v, l)| view! { <option value=v.clone() selected=v == chosen>{l}</option> }).collect_view()
            };
            view! { <label class="row" style="gap:6px"><span class="small muted">"User"</span><select class="input" id="r-user" style="width:auto;padding:5px 10px" aria-label="User" on:change=on_change>{options}</select></label> }.into_any()
        }
        Control::RetryFailed => {
            let retry = move |_| common::retry_failed(whose.get_untracked(), page.reload());
            view! { <button class="btn sm" id="retry-failed" title="Queue every failed request again" on:click={retry} {..html(format!("{}Retry failed", icon("refresh")))}>{""}</button> }.into_any()
        }
        Control::DownloadAllNow => {
            // every waiting request woken, the lines moved up (one person's with the User list)
            let all = move |_| common::download_all_now(whose.get_untracked(), page.reload());
            view! { <button class="btn sm" id="download-all-now" title=common::DOWNLOAD_ALL_NOW_TITLE on:click={all} {..html(format!("{}Download all now", icon("download")))}>{""}</button> }.into_any()
        }
        Control::ClearFinished => {
            let menu = move |e: web_sys::MouseEvent| {
                if let Some(b) = dom::target_el(&e).and_then(|t| dom::closest_of(&t, "#clear")) {
                    clear_menu(&b, page.reload(), whose.get_untracked());
                }
            };
            view! { <button class="btn sm" id="clear" on:click={menu} {..html(format!("{}Clear finished", icon("trash")))}>{""}</button> }.into_any()
        }
        Control::ClearWaiting => {
            // admins everyone's (or the person picked), members their own
            let clear = move |_| common::clear_waiting(whose.get_untracked(), page.reload());
            view! { <button class="btn sm" id="clear-waiting" title="Cancel every request that is still waiting" on:click={clear} {..html(format!("{}Clear waiting", icon("x")))}>{""}</button> }.into_any()
        }
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

/// The User list drawn again with the counts now (not while it is open, and only when they changed).
async fn paint_users(page: Page) {
    let Ok(users) = api::get("/requests/users").await else { return };
    let focused = dom::document().active_element().is_some_and(|a| a.id() == "r-user");
    if focused {
        return;
    }
    let options = user_options(&users.items(), page.picked.try_get_value().flatten().as_ref());
    if page.users.try_with_untracked(|old| *old != options) == Some(true) {
        page.users.set(options);
    }
}

/// The list's rows for the tab and whose requests show, into the page.
async fn load(page: Page) -> Result<(), String> {
    let filter = page.filter.get_untracked();
    let mut q = String::from("limit=300");
    if filter != "all" { q.push_str(&format!("&status={}", String::from(js_sys::encode_uri_component(&filter)))); }
    q.push_str(&page.whose.get_untracked().query());
    let rows = api::get(&format!("/requests?{q}")).await.map_err(|e| e.message)?.items();
    page.rows.try_set(Some(rows));
    Ok(())
}

/// The list as HTML: each request at the top level (a child whose parent is not listed counts as one), with its
/// children folded under it.
fn list_html(rows: &[Value], admin: bool) -> String {
    let top: Vec<&Value> = rows.iter().filter(|r| !r.truthy("parent_id") || !rows.iter().any(|p| p.g("id") == r.g("parent_id"))).collect();
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
}

#[cfg(test)]
mod tests {
    use super::{badges, controls, name_of, user_label, user_options, Control};
    use serde_json::json;

    /// Admins pick whose requests show in a User list ("All users", then each person with their count); members
    /// have no list (they only ever see their own).
    #[test]
    fn the_user_list() {
        assert_eq!(controls(true, true)[0], Control::User);
        assert!(!controls(false, true).contains(&Control::User));
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
        assert_eq!(crate::pages::common::DOWNLOAD_ALL_NOW_TITLE, "Wake every waiting request and move requests up in their lines – never more at once than each source allows");
        let admin = controls(true, true);
        let pos = |c| admin.iter().position(|x| *x == c).unwrap();
        assert_eq!(pos(Control::RetryFailed) + 1, pos(Control::DownloadAllNow), "beside Retry failed");
        assert!(!controls(false, true).contains(&Control::DownloadAllNow));
        assert!(controls(false, true).contains(&Control::RetryFailed));
        assert!(!controls(false, false).contains(&Control::RetryFailed));
    }

    /// Clear waiting: next to Clear finished for admins, and there for every member (their own requests).
    #[test]
    fn clear_waiting_is_next_to_clear_finished_and_for_everyone() {
        let admin = controls(true, true);
        let pos = |c| admin.iter().position(|x| *x == c).unwrap();
        assert_eq!(pos(Control::ClearFinished) + 1, pos(Control::ClearWaiting), "right after Clear finished");
        for member in [controls(false, true), controls(false, false)] {
            assert!(member.contains(&Control::ClearWaiting) && !member.contains(&Control::ClearFinished), "{member:?}");
        }
        assert_eq!(crate::pages::common::waiting_question(612), "Cancel 612 waiting requests? Downloads and imports already running keep going.");
        assert_eq!(crate::pages::common::waiting_question(1), "Cancel 1 waiting request? Downloads and imports already running keep going.");
        assert_eq!(crate::pages::common::waiting_done(612), "Cancelled 612 waiting requests");
        assert_eq!(crate::pages::common::waiting_done(1), "Cancelled 1 waiting request");
        assert_eq!(crate::pages::common::waiting_done(0), "Nothing is waiting");
    }

    /// The badges show only the groups that have requests.
    #[test]
    fn the_counts() {
        let c = json!({ "all": 20, "remaining": 12, "pending": 0, "active": 12, "completed": 6, "failed": 2 });
        assert_eq!(badges(&c), vec![("remaining", "info", "12 remaining".to_string()), ("failed", "err", "2 failed".to_string())]);
        assert!(badges(&json!({ "all": 4, "completed": 4 })).is_empty());
        assert_eq!(badges(&json!({ "pending": 1 }))[0].2, "1 awaiting approval");
    }
}
