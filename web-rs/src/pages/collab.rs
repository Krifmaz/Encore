//! Collaborative playlists (0.37): the owner's Collaborative switch and Invite window on a playlist's page,
//! a member's line there, the invites waiting on the Playlists page, and the invite link (`#/collab/<token>`).
//! docs/contracts/collaborative-playlists.md.
use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{confirm_dialog, esc, icon, modal, toast, ModalOpts};
use serde_json::{json, Value};
use std::rc::Rc;
use web_sys::Element;

pub const INVITE_TITLE: &str = "Invite collaborators";
pub const MEMBER_HINT: &str = "You can add, remove and reorder songs";
pub const LEAVE_TEXT: &str = "You stop seeing it and can't change it any more. The owner can invite you again.";

/// The user's role in a playlist: "owner" (collaborative now or before), "collaborator", or "".
pub fn role(p: &Value) -> String {
    p.g("collab").s("role")
}
/// Someone else's collaborative playlist the user joined.
pub fn is_member(p: &Value) -> bool {
    role(p) == "collaborator"
}
/// The owner's playlist with Collaborative on.
pub fn is_on(p: &Value) -> bool {
    p.truthy("collaborative")
}

/// " · collaborative" / " · by <owner>" on a playlist's tile.
pub fn tile_suffix(p: &Value) -> String {
    if is_member(p) {
        format!(" · by {}", esc(&p.g("collab").s("owner")))
    } else if is_on(p) {
        " · collaborative".to_string()
    } else {
        String::new()
    }
}

/// "1 member" / "3 members" ("" for none).
pub fn members_text(n: i64) -> String {
    match n {
        n if n <= 0 => String::new(),
        1 => "1 member".to_string(),
        n => format!("{n} members"),
    }
}

/// The line under a playlist's header: the owner's switch and Invite, or a member's badge, owner and
/// Members. Empty for Liked Songs.
pub fn line(pl: &Value) -> String {
    if pl.s("kind") != "playlist" {
        return String::new();
    }
    if is_member(pl) {
        return format!(r#"<div class="row wrap mt" id="pl-collab"><span class="badge accent">Collaborative</span><span class="small muted">by {owner} · {hint}</span><button class="btn sm" id="pl-members">{users}Members</button></div>"#,
            owner = esc(&pl.g("collab").s("owner")), hint = MEMBER_HINT, users = icon("user"));
    }
    let on = is_on(pl);
    let n = members_text(pl.g("collab").i("members"));
    format!(r#"<div class="row wrap mt" id="pl-collab"><label class="switch" title="Let people you invite add, remove and reorder songs"><input type="checkbox" id="pl-collab-on" {checked}><i></i>Collaborative</label>{extra}</div>"#,
        checked = if on { "checked" } else { "" },
        extra = if on { format!(r#"<span class="small muted">{n}</span><button class="btn sm" id="pl-invite">{}Invite</button>"#, icon("plus")) } else { String::new() })
}

/// The handlers of [`line`]; `reload` draws the page again.
pub fn bind(view: &Element, id: &str, reload: fn()) {
    {
        let id = id.to_string();
        dom::on_sel(view, "#pl-collab-on", "change", move |e| {
            let on = dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false);
            let id = id.clone();
            dom::spawn(async move {
                match api::patch(&format!("/playlists/{id}"), json!({ "collaborative": on })).await {
                    Ok(_) => {
                        toast(if on { "Collaborative: people you invite can change the songs" } else { "Collaborative off: members can't see it until it's on again" }, "ok");
                        reload();
                        if on { invite_modal(id, reload); }
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
    }
    {
        let id = id.to_string();
        dom::on_sel(view, "#pl-invite", "click", move |_| invite_modal(id.clone(), reload));
    }
    {
        let id = id.to_string();
        dom::on_sel(view, "#pl-members", "click", move |_| invite_modal(id.clone(), reload));
    }
}

/// Leave someone's collaborative playlist (the member's Delete), then back to Playlists.
pub async fn leave(id: &str, name: &str) {
    if !confirm_dialog("Leave playlist", &format!("Leave “{name}”? {LEAVE_TEXT}"), "Leave", true).await {
        return;
    }
    match api::del(&format!("/playlists/{id}")).await {
        Ok(_) => { toast("You left the playlist", "ok"); app::navigate("/playlists"); }
        Err(e) => toast(&e.message, "err"),
    }
}

/// The members window: for the owner, invite by user name, the invite link and the list (with remove);
/// for a member, who's in it and Leave.
fn invite_modal(id: String, reload: fn()) {
    let m = modal(ModalOpts::new(INVITE_TITLE, r#"<div id="cm-body"><div class="muted">Loading…</div></div>"#)
        .footer(r#"<button class="btn" data-close>Done</button>"#));
    fill(m, id, reload);
}

/// Loads the members and draws the window's body (again after each change).
fn fill(m: crate::ui::Modal, id: String, reload: fn()) {
    dom::spawn(async move {
        let Some(body) = m.qs("#cm-body") else { return };
        let v = match api::get(&format!("/playlists/{id}/collaborators")).await {
            Ok(v) => v,
            Err(e) => { dom::set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); return; }
        };
        let owner = v.s("role") == "owner";
        let members: String = v.a("members").iter().map(|p| format!(r#"<div class="row between" style="padding:6px 0;border-bottom:1px solid var(--line)"><span>{name}{badge}</span>{rm}</div>"#,
            name = esc(&p.s("username")),
            badge = if p.s("status") == "invited" { r#" <span class="badge">invited</span>"# } else { "" },
            rm = if owner { format!(r#"<button class="btn sm ghost" data-rm-member="{}" title="Remove">{}</button>"#, p.i("user_id"), icon("x")) } else { String::new() })).collect();
        let list = format!(r#"<div class="mt"><b>Members</b><div class="small muted">Owner: {}</div>{}</div>"#, esc(&v.s("owner")),
            if members.is_empty() { r#"<div class="small faint mt">Nobody yet.</div>"#.to_string() } else { members });
        let html = if owner {
            let link = v.g("invite_link");
            let link_row = if link.is_object() {
                format!(r#"<div class="row wrap mt"><input class="input grow" readonly value="{url}" id="cm-url"><button class="btn sm" id="cm-copy">{copy}Copy link</button><button class="btn sm ghost" id="cm-nolink">Turn off link</button></div>"#,
                    url = esc(&link_url(link)), copy = icon("copy"))
            } else {
                format!(r#"<div class="row wrap mt"><button class="btn sm" id="cm-link">{}Create invite link</button><span class="small faint">Anyone on this server with the link can join.</span></div>"#, icon("link"))
            };
            format!(r#"<p class="muted" style="margin-top:0">People you invite can add, remove and reorder songs. Only you can rename, share or delete the playlist.</p>
            <div class="row"><input class="input grow" id="cm-user" placeholder="User name on this server" autocomplete="off"><button class="btn primary" id="cm-add">{plus}Invite</button></div>
            <div id="cm-out"></div>{link_row}{list}"#, plus = icon("plus"))
        } else {
            format!(r#"<p class="muted" style="margin-top:0">{MEMBER_HINT}. Only {} can rename, share or delete it.</p>{list}<div class="row mt"><button class="btn danger ghost" id="cm-leave">Leave playlist</button></div>"#, esc(&v.s("owner")))
        };
        dom::set_html(&body, &html);

        let again = { let (m, id) = (m.clone(), id.clone()); move || { fill(m.clone(), id.clone(), reload); reload(); } };
        let invite = {
            let (m, id, again) = (m.clone(), id.clone(), again.clone());
            Rc::new(move || {
                let name = m.qs("#cm-user").map(|i| dom::value(&i)).unwrap_or_default();
                if name.trim().is_empty() { return; }
                let (m, id, again) = (m.clone(), id.clone(), again.clone());
                dom::spawn(async move {
                    match api::post(&format!("/playlists/{id}/collaborators"), json!({ "username": name.trim() })).await {
                        Ok(_) => { toast(&format!("Invited {}", name.trim()), "ok"); again(); }
                        Err(e) => if let Some(out) = m.qs("#cm-out") { dom::set_html(&out, &format!(r#"<div class="error-box mt">{}</div>"#, esc(&e.message))); },
                    }
                });
            })
        };
        { let f = invite.clone(); dom::on_sel(&body, "#cm-add", "click", move |_| f()); }
        if let Some(input) = m.qs("#cm-user") {
            let f = invite.clone();
            dom::on(&input, "keydown", move |e| { if dom::key(e) == "Enter" { dom::prevent(e); f(); } });
            dom::focus(&input);
        }
        {
            let (id, again) = (id.clone(), again.clone());
            dom::on_sel(&body, "#cm-link", "click", move |_| {
                let (id, again) = (id.clone(), again.clone());
                dom::spawn(async move {
                    match api::post_empty(&format!("/playlists/{id}/invite-link")).await {
                        Ok(_) => again(),
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            });
        }
        {
            let url = link_url(v.g("invite_link"));
            dom::on_sel(&body, "#cm-copy", "click", move |_| crate::ui::copy_text(&url));
        }
        {
            let (id, again) = (id.clone(), again.clone());
            dom::on_sel(&body, "#cm-nolink", "click", move |_| {
                let (id, again) = (id.clone(), again.clone());
                dom::spawn(async move {
                    if api::del(&format!("/playlists/{id}/invite-link")).await.is_ok() { toast("The invite link stopped working", "ok"); again(); }
                });
            });
        }
        {
            let (id, again) = (id.clone(), again.clone());
            dom::on_each(&body, "[data-rm-member]", "click", move |b, _| {
                let (id, again, uid) = (id.clone(), again.clone(), dom::data(b, "rm-member"));
                dom::spawn(async move {
                    if api::del(&format!("/playlists/{id}/collaborators/{uid}")).await.is_ok() { again(); }
                });
            });
        }
        {
            let (m, id, name) = (m.clone(), id.clone(), v.s("name"));
            dom::on_sel(&body, "#cm-leave", "click", move |_| {
                let (m, id, name) = (m.clone(), id.clone(), name.clone());
                dom::spawn(async move { m.close(); leave(&id, if name.is_empty() { "this playlist" } else { &name }).await; });
            });
        }
    });
}

/// The invite link to hand out: the server's address when the public URL is set, else this page's.
fn link_url(link: &Value) -> String {
    let url = link.s("url");
    if url.starts_with("http") { url } else { crate::ui::absolute_url(&link.s("path")).unwrap_or_default() }
}

// ---- the invited -------------------------------------------------------------------------------------

/// The invites waiting for the user, on top of the Playlists page (empty when there are none).
pub fn invites_html(list: &[Value]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let rows: String = list.iter().map(|i| format!(r#"<div class="row wrap between" style="padding:6px 0"><span><b>{owner}</b> invited you to collaborate on <b>{name}</b> <span class="small muted">· {n} tracks</span></span>
        <span class="row"><button class="btn sm primary" data-accept="{id}">Join</button><button class="btn sm ghost" data-decline="{id}">Decline</button></span></div>"#,
        owner = esc(&i.s("owner")), name = esc(&i.s("name")), n = i.i("count"), id = i.i("playlist_id"))).collect();
    format!(r#"<div class="card" id="collab-invites" style="margin-bottom:16px">{rows}</div>"#)
}

/// Join / Decline on [`invites_html`].
pub fn bind_invites(view: &Element, reload: fn()) {
    dom::on_each(view, "[data-accept]", "click", move |b, _| {
        let pid = dom::data(b, "accept");
        dom::spawn(async move {
            match api::post_empty(&format!("/collab/invites/{pid}/accept")).await {
                Ok(_) => { toast("Joined: you can change its songs now", "ok"); app::navigate(&format!("/playlists/{pid}")); }
                Err(e) => { toast(&e.message, "err"); reload(); }
            }
        });
    });
    dom::on_each(view, "[data-decline]", "click", move |b, _| {
        let pid = dom::data(b, "decline");
        dom::spawn(async move {
            let _ = api::post_empty(&format!("/collab/invites/{pid}/decline")).await;
            reload();
        });
    });
}

/// Join with an invite link pasted into Add a shared playlist: → the playlist.
pub async fn join_link(link: &str) -> Result<String, String> {
    let r = api::post("/collab/join", json!({ "link": link })).await.map_err(|e| e.message)?;
    toast(&format!("Joined {}'s playlist", r.s("owner")), "ok");
    Ok(r.s("id"))
}

/// A pasted text that is an invite link rather than a share link.
pub fn is_invite_link(text: &str) -> bool {
    text.contains("collab/")
}

/// `#/collab/<token>`: the Playlists page with "Join <owner>'s playlist?" over it.
pub fn render_link(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let token = ctx.param(0);
        if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("#/playlists")); }
        dom::spawn(async move { ask_join(token).await; });
        crate::pages::playlists::render(view, Ctx { params: vec![], ..ctx }).await
    })
}

async fn ask_join(token: String) {
    let c = match api::post("/collab/join/check", json!({ "link": token })).await {
        Ok(c) => c,
        Err(e) => { toast(&e.message, "err"); return; }
    };
    if c.truthy("own") || c.truthy("joined") {
        app::navigate(&format!("/playlists/{}", c.s("id")));
        return;
    }
    let text = format!("{} invited you to collaborate on “{}” ({} tracks). You'll be able to add, remove and reorder its songs.", c.s("owner"), c.s("name"), c.i("count"));
    if !confirm_dialog("Join playlist", &text, "Join", false).await {
        return;
    }
    match join_link(&token).await {
        Ok(id) => app::navigate(&format!("/playlists/{id}")),
        Err(e) => toast(&e, "err"),
    }
}
