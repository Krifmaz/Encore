//! What a group listen looks like in the web app for accounts: "Start a group listen", the group's dialog (the code,
//! Copy invite link, the people and their devices, the host's settings, End for everyone / Leave), "Join a group
//! listen" with a code box, "Join <host>'s group listen?" for a code or an invite link (`#/group/<code>`), Home's
//! "Listening together now" row and the devices panel's "Group listens" section. The words are client-shapes §5.7.

use super::{rules, OnGroup};
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{self, esc, icon, modal, toast, Modal, ModalOpts};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// A member's or a host's picture: the server's avatar path, else the first letter of the name.
fn avatar(path: &str, name: &str, size: u32) -> String {
    let style = format!(r#" style="width:{size}px;height:{size}px;font-size:{}px""#, (size as f64 * 0.42).round() as i64);
    if !path.is_empty() && (path.starts_with('/') || path.starts_with("https://") || path.starts_with("http://")) {
        format!(r#"<img class="avatar" src="{}" alt=""{style}>"#, esc(path))
    } else {
        format!(r#"<div class="avatar"{style}>{}</div>"#, esc(&ui::initials(name)))
    }
}

/// The cover of a group's song (a library album's cover).
fn cover(track: &Value, size: u32) -> String {
    if !track.truthy("album_id") { return ui::art("", &track.s("title"), ""); }
    ui::art_sized(&format!("/api/library/albums/{}/cover{}", track.s("album_id"), ui::cover_q(track)), &track.s("title"), "", size)
}

/// Sets `el`'s HTML unless it already shows exactly that (a group's change repaints these views; their covers and
/// pictures are not drawn again for nothing).
fn set_if_changed(el: &Element, html: &str) {
    if dom::data_opt(el, "drawn").as_deref() == Some(&*hash(html)) { return; }
    dom::set_data(el, "drawn", &hash(html));
    dom::set_html(el, html);
}
fn hash(s: &str) -> String {
    // FNV-1a: only to tell one drawing from the next
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    format!("{h:x}")
}

/// Keeps `f` subscribed to the group's changes while `el` is on screen.
fn follow_while(el: &Element, f: impl Fn() + 'static) {
    let held: Rc<RefCell<Option<OnGroup>>> = Rc::new(RefCell::new(None));
    let (el2, h2) = (el.clone(), held.clone());
    *held.borrow_mut() = Some(super::on_change(move || {
        // gone from the page: unsubscribe after this round
        if !el2.is_connected() { let h = h2.clone(); dom::spawn(async move { h.borrow_mut().take(); }); return; }
        f();
    }));
}

/// Copies the invite link ("Invite link copied" / "Couldn't copy the link").
fn copy_link(link: &str) {
    let done = |ok: bool| toast(if ok { "Invite link copied" } else { "Couldn't copy the link" }, if ok { "ok" } else { "err" });
    let by_selection = |t: &str| -> bool {
        let Ok(area) = dom::create("textarea").dyn_into::<web_sys::HtmlTextAreaElement>() else { return false };
        area.set_value(t);
        let _ = area.set_attribute("readonly", "");
        let _ = area.set_attribute("style", "position:fixed;top:-1000px;left:0;width:1px;height:1px;opacity:0");
        let host: Element = dom::qs("#modal-root .modal-bg:last-child .modal").unwrap_or_else(|| dom::body().into());
        let _ = host.append_child(&area);
        area.select();
        let _ = area.set_selection_range(0, t.encode_utf16().count() as u32);
        let doc = dom::document();
        let ok = js_sys::Reflect::get(&doc, &"execCommand".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .and_then(|f| f.call1(&doc, &"copy".into()).ok()).and_then(|r| r.as_bool()).unwrap_or(false);
        area.remove();
        ok
    };
    let has_api = js_sys::Reflect::get(&dom::window().navigator(), &"clipboard".into()).map(|c| !c.is_undefined() && !c.is_null()).unwrap_or(false);
    if !has_api { done(by_selection(link)); return; }
    let t = link.to_string();
    dom::spawn(async move {
        let ok = wasm_bindgen_futures::JsFuture::from(dom::window().navigator().clipboard().write_text(&t)).await.is_ok() || by_selection(&t);
        done(ok);
    });
}

fn origin() -> String { dom::location().origin().unwrap_or_default() }

// ---- Start ---------------------------------------------------------------------------------------------------------

/// "Start a group listen": who can join and whether everyone controls; what plays here becomes the group's queue.
pub fn start_dialog() {
    let m = modal(ModalOpts::new("Start a group listen", r#"<p class="small muted" style="margin:0 0 12px">Everyone who joins hears the same songs at the same time, each on their own device. What plays here now becomes the group's queue.</p>
      <div class="stack" style="gap:6px">
        <div style="font-weight:600">Who can join</div>
        <label class="row" style="gap:8px;cursor:pointer"><input type="radio" name="g-vis" value="anyone" checked><span>Anyone on this server can see and join</span></label>
        <label class="row" style="gap:8px;cursor:pointer"><input type="radio" name="g-vis" value="link"><span>Only people with the link</span></label>
        <label class="switch" style="margin-top:8px"><input type="checkbox" id="g-everyone"><i></i><span>Everyone can control</span></label>
        <div class="small muted">Anyone in the group can play, pause, skip and change the queue.</div>
      </div>"#)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="g-start">Start</button>"#));
    let Some(btn) = m.qs("#g-start") else { return };
    let m2 = m;
    dom::on(&btn, "click", move |_| {
        if super::busy() { return; }
        let vis = m2.qsa("[name=g-vis]").into_iter().find(dom::checked).map(|r| dom::value(&r)).unwrap_or_else(|| "anyone".into());
        let everyone = m2.qs("#g-everyone").map(|c| dom::checked(&c)).unwrap_or(false);
        let (m3, b) = (m2.clone(), m2.qs("#g-start"));
        if let Some(b) = &b { dom::set_disabled(b, true); dom::set_text(b, "Starting…"); }
        dom::spawn(async move {
            match super::start_group(&vis, everyone).await {
                Ok(()) => { m3.close(); sheet(); }
                Err(e) => {
                    if let Some(b) = &b { dom::set_disabled(b, false); dom::set_text(b, "Start"); }
                    if !e.is_empty() { toast(&e, "err"); }
                }
            }
        });
    });
}

// ---- the group's dialog --------------------------------------------------------------------------------------------

/// The group listen this browser is in: the code and invite link, the people, the host's settings, End / Leave.
pub fn sheet() {
    if super::current().is_none() { return; }
    // one at a time
    if dom::qs("#g-sheet").is_some() { return; }
    let m = modal(ModalOpts::new("Group listen", r#"<div id="g-sheet" class="stack" style="gap:14px"></div>"#));
    let Some(root) = m.qs("#g-sheet") else { return };
    // the limit the host is tapping towards, sent once the taps stop
    let limit: Rc<Cell<Option<i64>>> = Rc::new(Cell::new(None));
    let timer: Rc<RefCell<Option<dom::Timeout>>> = Rc::new(RefCell::new(None));
    let admin = app::is_admin();
    let paint: Rc<dyn Fn()> = {
        let (root, m, limit) = (root.clone(), m.clone(), limit.clone());
        Rc::new(move || {
            let Some(s) = super::current() else { m.close(); return };
            paint_sheet(&root, &s, admin, limit.get());
        })
    };
    paint();
    {
        let p = paint.clone();
        follow_while(&root, move || p());
    }
    let (m2, p2) = (m, paint.clone());
    dom::on(&root, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-g]") else { return };
        let Some(s) = super::current() else { return };
        match dom::data(&b, "g").as_str() {
            "copy" => copy_link(&rules::invite_link(&s, &origin())),
            "catchup" => super::catch_up(),
            "pause-all" => crate::player::pause_for_everyone(),
            "leave" => { super::leave(); m2.close(); }
            "end" => {
                let m3 = m2.clone();
                dom::spawn(async move {
                    if ui::confirm_dialog("End the group listen?", "The music stops for everyone in it.", "End for everyone", true).await {
                        super::end();
                        m3.close();
                    }
                });
            }
            "member" => {
                let (member, name) = (dom::data(&b, "member"), dom::data(&b, "name"));
                ui::pop_menu(&b, vec![ui::MenuItem::danger("Remove from the group", &icon("x"), move || super::remove_member(&member, &name))]);
            }
            "minus" | "plus" => {
                let cap = s.i("server_max_members").max(2);
                let floor = (s.a("members").len() as i64).max(2);
                let now = limit.get().unwrap_or(s.i("max_members").max(2));
                let next = if dom::data(&b, "g") == "minus" { (now - 1).max(floor) } else { (now + 1).min(cap) };
                limit.set(Some(next));
                p2();
                let l2 = limit.clone();
                *timer.borrow_mut() = Some(dom::Timeout::new(700, move || {
                    if let Some(n) = l2.take() { if super::current().is_some_and(|s| s.i("max_members") != n) { super::patch(json!({ "max_members": n })); } }
                }));
            }
            _ => {}
        }
    });
    dom::on(&root, "change", move |e| {
        let Some(t) = dom::target_el(e) else { return };
        match t.id().as_str() {
            "g-everyone" => super::patch(json!({ "everyone_controls": dom::checked(&t) })),
            "g-guests" => super::patch(json!({ "guests_allowed": dom::checked(&t) })),
            _ if t.get_attribute("name").as_deref() == Some("g-vis") && dom::checked(&t) => super::patch(json!({ "visibility": dom::value(&t) })),
            _ => {}
        }
    });
}

fn paint_sheet(root: &Element, s: &Value, admin: bool, pending_limit: Option<i64>) {
    let you = s.g("you");
    let host = rules::host_name(s);
    let intro = if you.truthy("host") { "You host this group listen. Everyone hears the same songs at the same time, each on their own device.".to_string() }
        else { format!("{host} hosts this group listen. Everyone hears the same songs at the same time, each on their own device.") };
    let members = s.a("members");
    let people: String = members.iter().map(|m| {
        let badges = format!("{}{}{}",
            if m.truthy("host") { r#"<span class="badge accent">Host</span>"# } else { "" },
            if m.truthy("guest") { r#"<span class="badge">Guest</span>"# } else { "" },
            if m.s("member") == you.s("member") { r#"<span class="badge">You</span>"# } else { "" });
        let remove = if (you.truthy("host") || admin) && m.truthy("guest") {
            format!(r#"<button class="btn ghost icon sm" data-g="member" data-member="{}" data-name="{}" aria-label="More" title="More">{}</button>"#, esck(m, "member"), esck(m, "name"), icon("more"))
        } else { String::new() };
        format!(r#"<div class="list-item">{av}<div class="grow" style="min-width:0"><div class="row" style="gap:6px;min-width:0"><b class="ellipsis">{name}</b>{badges}</div>{device}</div>{remove}</div>"#,
            av = avatar(&m.s("avatar"), &m.s("name"), 32), name = esck(m, "name"),
            device = if m.truthy("device_name") { format!(r#"<div class="small muted ellipsis">{}</div>"#, esck(m, "device_name")) } else { String::new() })
    }).collect();
    let settings = if you.truthy("host") {
        let cap = s.i("server_max_members").max(2);
        let lim = pending_limit.unwrap_or(s.i("max_members").max(2));
        let floor = (members.len() as i64).max(2);
        let link_only = s.s("visibility") == "link";
        let guests = if s.truthy("guests_possible") {
            format!(r#"<label class="switch"><input type="checkbox" id="g-guests"{}><i></i><span>Guests can join with the link</span></label>
            <div class="small muted">People without an account join on the web with the invite link. They listen; they can't add songs or control playback.</div>"#,
                if s.truthy("guests_allowed") { " checked" } else { "" })
        } else { String::new() };
        format!(r#"<div class="stack" style="gap:6px">
          <h3 style="margin:4px 0 2px">Settings</h3>
          <label class="switch"><input type="checkbox" id="g-everyone"{everyone}><i></i><span>Everyone can control</span></label>
          <div class="small muted">Anyone in the group can play, pause, skip and change the queue.</div>
          <div style="font-weight:600;margin-top:6px">Who can join</div>
          <label class="row" style="gap:8px;cursor:pointer"><input type="radio" name="g-vis" value="anyone"{anyone}><span>Anyone on this server can see and join</span></label>
          <label class="row" style="gap:8px;cursor:pointer"><input type="radio" name="g-vis" value="link"{link}><span>Only people with the link</span></label>
          {guests}
          <div class="row between" style="margin-top:6px"><span>Limit to {lim} people</span>
            <span class="row" style="gap:6px"><button class="btn sm" data-g="minus" aria-label="−"{minus}>−</button><button class="btn sm" data-g="plus" aria-label="+"{plus}>+</button></span></div>
          <div class="small muted">At most {cap} on this server.</div>
        </div>"#,
            everyone = if s.truthy("everyone_controls") { " checked" } else { "" },
            anyone = if link_only { "" } else { " checked" }, link = if link_only { " checked" } else { "" },
            minus = if lim <= floor { " disabled" } else { "" }, plus = if lim >= cap { " disabled" } else { "" })
    } else {
        format!(r#"<div class="small muted">{}</div>"#, esc(&if s.truthy("everyone_controls") { "Everyone can control playback.".to_string() } else { format!("{}. You can add songs with Add to queue.", rules::only_host(s)) }))
    };
    let paused = if super::local_paused() {
        r#"<div class="row between card flat" style="padding:10px 12px"><span>Paused here</span><button class="btn sm primary" data-g="catchup">Catch up</button></div>"#
    } else { "" };
    // focus and the scroll stay where they were when the dialog is drawn again
    let focused = dom::document().active_element().map(|a| a.id()).unwrap_or_default();
    set_if_changed(root, &format!(r#"<p class="small muted" style="margin:0">{intro}</p>
      <div class="card group-code">
        <div class="small muted">Code</div>
        <div class="group-code-v">{code}</div>
        <button class="btn sm" data-g="copy">{copy}Copy invite link</button>
      </div>
      {paused}
      <div><h3 style="margin:0 0 6px">People ({n})</h3><div class="list">{people}</div></div>
      {settings}
      <div class="row wrap" style="gap:8px;justify-content:flex-end">{pause_all}{end}<button class="btn" data-g="leave">Leave</button></div>"#,
        intro = esc(&intro), code = esc(&rules::spaced(&s.s("code"))), copy = icon("copy"), n = members.len(),
        // the group's own pause (Pause in the player pauses only here), for those who control it while it plays
        pause_all = if rules::offers_pause_for_everyone(s) { format!(r#"<button class="btn" data-g="pause-all">{}{}</button>"#, icon("pause"), rules::PAUSE_FOR_EVERYONE) } else { String::new() },
        end = if you.truthy("host") { r#"<button class="btn danger" data-g="end">End for everyone</button>"# } else { "" }));
    if !focused.is_empty() { if let Some(el) = dom::by_id(&focused) { dom::focus(&el); } }
}

fn esck(v: &Value, k: &str) -> String { esc(&v.s(k)) }

// ---- Join ----------------------------------------------------------------------------------------------------------

/// "Join a group listen": a code, or an invite link pasted whole.
pub fn join_dialog() {
    let m = modal(ModalOpts::new("Join a group listen", r#"<p class="small muted" style="margin:0 0 10px">Type the code the host gave you, or paste the invite link.</p>
      <form id="g-join-form"><input class="input mono" id="g-code" placeholder="Code" autocomplete="off" autocapitalize="characters" spellcheck="false" maxlength="300" style="width:100%"></form>"#)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="g-join">Join</button>"#));
    if let Some(i) = m.qs("#g-code") { dom::focus(&i); }
    let go: Rc<dyn Fn()> = {
        let m = m.clone();
        Rc::new(move || {
            let text = m.qs("#g-code").map(|i| dom::value(&i)).unwrap_or_default();
            if open_invite(&text) { m.close(); }
        })
    };
    if let Some(b) = m.qs("#g-join") { let g = go.clone(); dom::on(&b, "click", move |_| g()); }
    if let Some(f) = m.qs("#g-join-form") {
        let g = go.clone();
        dom::keep(dom::listen(&f, "submit", move |e| { dom::prevent(e); g(); }));
    }
}

/// A code or an invite link to join (typed, or opened as `#/group/<code>`): who hosts it is looked up first and the
/// app asks "Join <host>'s group listen?". A link to another server is refused. False when the text was refused.
pub fn open_invite(text: &str) -> bool {
    let Some(code) = rules::code_from(text) else { toast("That isn't a group listen code or link", "err"); return false };
    if let Some(server) = rules::server_of(text) {
        let host = rules::host_of(&server);
        if !host.eq_ignore_ascii_case(&dom::location().host().unwrap_or_default()) {
            toast(&format!("That group listen is on another server ({host})"), "err");
            return false;
        }
    }
    if super::current().is_some_and(|s| s.s("code") == code) { sheet(); return true; }
    dom::spawn(async move {
        match super::lookup(&code).await {
            Ok(g) => invite_dialog(&code, Some(g)),
            Err(e) if e.status == 404 => toast("No group listen with that code", "err"),
            Err(e) if matches!(e.status, 403 | 429) => toast(&e.message, "err"),
            // the lookup failed otherwise: ask without the host's name, the join says the rest
            Err(_) => invite_dialog(&code, None),
        }
    });
    true
}

/// "Join <host>'s group listen?" with what plays there.
fn invite_dialog(code: &str, g: Option<Value>) {
    let title = rules::join_title(g.as_ref());
    let playing = g.as_ref().map(rules::playing_now).unwrap_or_default();
    let text = format!("{}{}What plays here stops, and you hear the group's songs with everyone else.", playing, if playing.is_empty() { "" } else { " " });
    let m = modal(ModalOpts::new(&esc(&title), &format!(r#"<p class="muted" style="margin:0">{}</p>"#, esc(&text)))
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="g-yes">Join</button>"#));
    let Some(b) = m.qs("#g-yes") else { return };
    let (m2, code, b2) = (m, code.to_string(), b.clone());
    dom::on(&b, "click", move |_| {
        if super::busy() { return; }
        dom::set_disabled(&b2, true);
        dom::set_text(&b2, "Joining…");
        let (m3, code, b3) = (m2.clone(), code.clone(), b2.clone());
        dom::spawn(async move {
            match super::join_code(&code).await {
                Ok(()) => m3.close(),
                Err(e) => {
                    dom::set_disabled(&b3, false);
                    dom::set_text(&b3, "Join");
                    if !e.is_empty() { toast(&e, "err"); }
                }
            }
        });
    });
}

/// Join a group of the server's list, or open the one this browser is in.
fn join_from_list(id: String, btn: Option<Element>) {
    if super::current().is_some_and(|s| s.s("id") == id) { sheet(); return; }
    if super::busy() { return; }
    if let Some(b) = &btn { dom::set_disabled(b, true); dom::set_text(b, "Joining…"); }
    dom::spawn(async move {
        let r = super::join_id(&id).await;
        if let Some(b) = &btn { dom::set_disabled(b, false); dom::set_text(b, "Join"); }
        if let Err(e) = r { if !e.is_empty() { toast(&e, "err"); } }
    });
}

// ---- the server's list: Home and the devices panel ------------------------------------------------------------------

/// Home's "Listening together now": a tile per live group listen this account may see (the host's picture on the
/// song's cover, "Alex · 3 listening", Join), refreshed every 20 s; nothing when there is none.
pub fn home_row(view: &Element) {
    let Some(el) = dom::qs_in(view, "#groups-now") else { return };
    let paint: Rc<dyn Fn()> = {
        let el = el.clone();
        Rc::new(move || {
            let list = super::live();
            if list.is_empty() || super::available() == Some(false) { set_if_changed(&el, ""); return; }
            let mine = super::current().map(|s| s.s("id")).unwrap_or_default();
            set_if_changed(&el, &format!(r#"<div class="section-head"><h2>Listening together now</h2></div><div class="grid">{}</div>"#,
                list.iter().map(|g| {
                    let t = g.g("track");
                    let line = [t.s("title"), t.s("artist")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" – ");
                    let open = g.s("id") == mine;
                    format!(r#"<div class="tile group-tile">
                      <div class="art">{art}<div class="group-host">{av}</div></div>
                      <div class="t ellipsis" title="{tl}">{tl}</div>
                      <div class="s ellipsis">{line}</div>
                      <button class="btn sm {cls}" data-gjoin="{id}" style="margin-top:6px">{act}</button></div>"#,
                        art = cover(t, 400), av = avatar(&g.g("host").s("avatar"), &g.g("host").s("name"), 40),
                        tl = esc(&rules::tile_line(g)), line = esc(&line), id = esc(&g.s("id")),
                        cls = if open { "" } else { "primary" }, act = if open { "Open" } else { "Join" })
                }).collect::<String>()));
        })
    };
    paint();
    {
        let p = paint.clone();
        follow_while(&el, move || p());
    }
    dom::on(&el, "click", |e| {
        let Some(b) = dom::closest(e, "[data-gjoin]") else { return };
        join_from_list(dom::data(&b, "gjoin"), Some(b));
    });
    dom::spawn(super::refresh_list());
    dom::interval(view, 20000, || dom::spawn(super::refresh_list()));
}

/// The devices panel's "Group listens": the group this browser is in, the others on the server, "Start a group
/// listen" and "Join with a code". Nothing on a server without group listens.
pub fn devices_section(el: &Element, panel: &Modal) {
    let paint: Rc<dyn Fn()> = {
        let el = el.clone();
        Rc::new(move || {
            if super::available() == Some(false) { set_if_changed(&el, ""); return; }
            let cur = super::current();
            let mut rows = String::new();
            let line = |host: &Value, title: &str, count: i64, status: &str, action: &str, id: &str| format!(
                r#"<div class="list-item">{av}<div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{who}</div><div class="small muted ellipsis">{sub}</div></div><button class="btn sm {cls}" data-gjoin="{id}">{action}</button></div>"#,
                av = avatar(&host.s("avatar"), &host.s("name"), 32),
                who = esc(&rules::tile_line(&json!({ "host": host, "members_count": count }))),
                sub = esc(&[title.to_string(), status.to_string()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")),
                cls = if action == "Join" { "primary" } else { "" }, id = esc(id));
            if let Some(s) = &cur {
                let t = s.g("track");
                rows.push_str(&line(s.g("host"), &t.s("title"), s.a("members").len() as i64, if s.g("you").truthy("host") { "You host it" } else { "You're in it" }, "Open", &s.s("id")));
            }
            for g in super::live().iter().filter(|g| cur.as_ref().is_none_or(|s| s.s("id") != g.s("id"))) {
                rows.push_str(&line(g.g("host"), &g.g("track").s("title"), g.i("members_count"), if g.truthy("mine") { "You're in it on another device" } else { "" }, "Join", &g.s("id")));
            }
            if rows.is_empty() { rows = r#"<p class="small muted" style="margin:0">No group listens on this server right now.</p>"#.to_string(); }
            set_if_changed(&el, &format!(r#"<h3 style="margin:0">Group listens</h3><div class="list">{rows}</div>
              <div class="row wrap" style="gap:8px">{start}<button class="btn sm" data-gact="join">Join with a code</button></div>"#,
                start = if cur.is_none() { format!(r#"<button class="btn sm primary" data-gact="start">{}Start a group listen</button>"#, icon("users")) } else { String::new() }));
        })
    };
    paint();
    {
        let p = paint.clone();
        follow_while(el, move || p());
    }
    let panel = panel.clone();
    dom::on(el, "click", move |e| {
        if let Some(b) = dom::closest(e, "[data-gjoin]") {
            let id = dom::data(&b, "gjoin");
            if super::current().is_some_and(|s| s.s("id") == id) { panel.close(); sheet(); return; }
            join_from_list(id, Some(b));
            return;
        }
        let Some(b) = dom::closest(e, "[data-gact]") else { return };
        panel.close();
        if dom::data(&b, "gact") == "start" { start_dialog(); } else { join_dialog(); }
    });
    dom::spawn(super::refresh_list());
    let el2 = el.clone();
    let t = dom::Interval::new(20000, move || { if el2.is_connected() { dom::spawn(super::refresh_list()); } });
    dom::keep(t);
}

// ---- #/group/<code> -------------------------------------------------------------------------------------------------

/// An invite link opened while signed in: Home, with "Join <host>'s group listen?" on top (the address goes back to
/// Home, so a reload doesn't ask again).
pub fn render_link(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let code = ctx.param(0);
        if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("#/")); }
        for a in dom::qsa("[data-nav]") { dom::toggle_class(&a, "active", dom::data(&a, "nav") == "/"); }
        let link = format!("#/group/{code}");
        dom::spawn(async move { open_invite(&link); });
        crate::pages::home::render(view, Ctx { params: vec![], ..ctx }).await
    })
}
