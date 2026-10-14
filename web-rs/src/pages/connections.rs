//! Port of web/js/pages/connections.js – see docs/ui-rewrite/CONTRACT.md
//! The Connections settings page: one card per service, the connect forms (user/password, token, Deezer profile,
//! Plex PIN, YouTube Music device code / headers, Last.fm), remote playlist import and "Sync now".

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::pages::playlists::import_modal;
use crate::ui::{confirm_dialog, esc, fmt, icon, modal, toast, ModalOpts};
use serde_json::Value;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// `INFO[k]`: what a service does and which connect form it uses.
struct Info {
    what: &'static str,
    form: &'static str,
}

/// `INFO[k] || {}` (`what` and `form` are "" when the service is unknown).
fn info(k: &str) -> Info {
    let (what, form) = match k {
        "jellyfin" => ("Two-way favourites (Finamp ♥) and playlist sync", "userpass"),
        "plex" => ("Two-way loves (Plexamp ♥) and playlist sync", "plex"),
        "navidrome" => ("Two-way stars and playlist sync", "userpass"),
        "ytmusic" => ("Two-way likes (👍) and playlist sync; import your library & Liked Music", "ytmusic"),
        "spotify" => ("Import private playlists & Liked Songs; likes flow into Encore", "oauth"),
        "tidal" => ("Import playlists & collection; likes flow into Encore", "oauth"),
        "deezer" => ("Import your public favourites & playlists", "profile"),
        "listenbrainz" => ("Scrobbles what you play here; two-way loves; playlists pushed (needs MusicBrainz-tagged tracks); For you recommendations", "token"),
        "lastfm" => ("Scrobbles what you play here; imports loved tracks; For you recommendations", "lastfm"),
        _ => ("", ""),
    };
    Info { what, form }
}

/// `window.dispatchEvent(new HashChangeEvent('hashchange'))` – re-renders the page.
fn hashchange() {
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") {
        let _ = dom::window().dispatch_event(&ev);
    }
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let connected = ctx.q("connected");
        if !connected.is_empty() { toast(&format!("{connected} connected"), "ok"); }
        let error = ctx.q("error");
        if !error.is_empty() { toast(&error, "err"); }
        let d = api::get("/connections").await.map_err(|e| e.message)?;
        let sync = d.g("sync");
        let last_sync = if sync.truthy("last") { format!(" Last sync {}.", fmt::ago(sync.f("last"))) } else { String::new() };
        let cards: String = d.g("services").as_object()
            .map(|m| m.iter().map(|(k, s)| card(k, s, d.g("connections").g(k), d.g("available").truthy(k))).collect())
            .unwrap_or_default();
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Connections</h1><div class="small muted">Link your accounts so likes and playlists stay the same in Finamp, Plexamp, Navidrome and here.{last_sync}</div></div>
      <button class="btn primary" id="sync-now">{}Sync now</button>
    </div>
    <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(300px,1fr))">
      {cards}
    </div>"#, icon("refresh")));
        if let Some(b) = dom::qs_in(&view, "#sync-now") {
            dom::on(&b, "click", move |e| {
                let Some(t) = dom::target_el(e) else { return };
                dom::spawn(async move {
                    dom::set_disabled(&t, true);
                    match api::post_empty("/sync").await {
                        Ok(r) => {
                            let errors = r.truthy("errors");
                            toast(&format!("Synced: +{} / −{} likes, {} changes pushed, {} playlist(s){}",
                                r.s("liked_added"), r.s("liked_removed"), r.s("pushed"), r.s("playlists"),
                                if errors { format!(" · {} error(s)", r.s("errors")) } else { String::new() }),
                                if errors { "err" } else { "ok" });
                            hashchange();
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&t, false);
                });
            });
        }
        let d = Rc::new(d);
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        if let Some(grid) = dom::qs_in(&view, ".grid") {
            let d2 = d.clone();
            dom::on(&grid, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-act]") else { return };
                let Some(card_el) = dom::closest_of(&b, "[data-svc]") else { return };
                let svc = dom::data(&card_el, "svc");
                let act = dom::data(&b, "act");
                if act == "connect" { connect(&svc, &d2, admin); return; }
                if act == "disconnect" {
                    let d = d2.clone();
                    dom::spawn(async move {
                        if !confirm_dialog("Disconnect", &format!("Disconnect {}?", d.g("services").g(&svc).s("label")), "Disconnect", true).await { return; }
                        if api::del(&format!("/connections/{svc}")).await.is_ok() { hashchange(); }
                    });
                    return;
                }
                if act == "playlists" { remote_playlists(&svc, &d2.g("services").g(&svc).s("label")); }
            });
            dom::on(&grid, "change", move |e| {
                let Some(t) = dom::target_el(e) else { return };
                let f = dom::data(&t, "flag");
                if f.is_empty() { return; }
                let Some(card_el) = dom::closest_of(&t, "[data-svc]") else { return };
                let svc = dom::data(&card_el, "svc");
                let checked = dom::checked(&t);
                dom::spawn(async move {
                    let mut body = serde_json::Map::new();
                    body.insert(f, Value::Bool(checked));
                    let _ = api::patch(&format!("/connections/{svc}"), Value::Object(body)).await;
                });
            });
        }
        Ok(())
    })
}

/// `card(k, s, c, available)`: one service card (`c` is the connection or Null).
fn card(k: &str, s: &Value, c: &Value, available: bool) -> String {
    let info = info(k);
    let connected = crate::json::truthy(c);
    let badge = if connected {
        if c.truthy("last_error") { format!(r#"<span class="badge err" title="{}">error</span>"#, esc(&c.s("last_error"))) }
        else { r#"<span class="badge ok">connected</span>"#.to_string() }
    } else { String::new() };
    let body = if connected {
        let synced = if c.truthy("last_sync") { format!(" · synced {}", fmt::ago(c.f("last_sync"))) } else { String::new() };
        let err_line = if c.truthy("last_error") { format!(r#"<div class="small" style="color:var(--err)">{}</div>"#, esc(&c.s("last_error"))) } else { String::new() };
        let scrobble = if c.truthy("can_scrobble") {
            format!(r#"<label class="switch"><input type="checkbox" data-flag="scrobble" {}><i></i>Scrobble what I play</label>"#, if c.truthy("scrobble") { "checked" } else { "" })
        } else { String::new() };
        let playlists = if s.s("kind") != "stream" {
            format!(r#"<label class="switch"><input type="checkbox" data-flag="sync_playlists" {}><i></i>Sync playlists</label>"#, if c.truthy("sync_playlists") { "checked" } else { "" })
        } else { String::new() };
        // `k === 'lastfm' && !c.can_scrobble && c.can_scrobble !== undefined`
        let lastfm_hint = if k == "lastfm" && !c.truthy("can_scrobble") && c.has("can_scrobble") {
            r#"<div class="small muted">Connected by username only – <button class="btn sm ghost" data-act="connect" style="padding:2px 6px">Sign in with Last.fm</button> to scrobble.</div>"#
        } else { "" };
        let import_btn = if k != "lastfm" { format!(r#"<button class="btn sm" data-act="playlists">{}Import playlists</button>"#, icon("download")) } else { String::new() };
        format!(r#"<div class="small">Signed in as <b>{}</b>{synced}</div>
      {err_line}
      <div class="row wrap">
        {scrobble}
        <label class="switch"><input type="checkbox" data-flag="sync_likes" {}><i></i>Sync likes</label>
        {playlists}
      </div>
      {lastfm_hint}
      <div class="row wrap">{import_btn}
        <button class="btn sm danger ghost" data-act="disconnect">Disconnect</button></div>"#,
            esc(&c.s("account")), if c.truthy("sync_likes") { "checked" } else { "" })
    } else if available {
        format!(r#"<button class="btn" data-act="connect">{}Connect</button>"#, icon("link"))
    } else {
        r#"<div class="small faint">Not set up yet – an admin needs to add it in Settings.</div>"#.to_string()
    };
    format!(r#"<div class="card stack" data-svc="{k}">
    <div class="row between"><h3>{}</h3>{badge}</div>
    <div class="small muted">{}</div>
    {body}
  </div>"#, esc(&s.s("label")), esc(info.what))
}

/// `Object.fromEntries(new FormData(form))` as a JSON object (every value a string).
fn form_entries(form: &Element) -> Value {
    let mut o = serde_json::Map::new();
    if let Some(f) = form.dyn_ref::<web_sys::HtmlFormElement>() {
        if let Ok(fd) = web_sys::FormData::new_with_form(f) {
            for e in fd.entries().into_iter().flatten() {
                if let Ok(a) = e.dyn_into::<js_sys::Array>() {
                    o.insert(a.get(0).as_string().unwrap_or_default(), Value::String(a.get(1).as_string().unwrap_or_default()));
                }
            }
        }
    }
    Value::Object(o)
}

/// `connect(svc, d, admin)`: start the right sign-in flow for the service (`d` is the `/connections` answer).
fn connect(svc: &str, d: &Value, admin: bool) {
    let form = info(svc).form;
    // `d.services[svc]?.label || svc`
    let label = { let l = d.g("services").g(svc).s("label"); if l.is_empty() { svc.to_string() } else { l } };
    if form == "oauth" {
        let svc = svc.to_string();
        // `(d.base_url || location.origin).replace(/\/+$/, '')`
        let base = { let b = d.s("base_url"); if b.is_empty() { dom::location().origin().unwrap_or_default() } else { b } };
        let base = base.trim_end_matches('/').to_string();
        dom::spawn(async move {
            match api::post(&format!("/connections/{svc}"), json!({})).await {
                Ok(r) => { let _ = dom::location().set_href(&r.s("url")); }
                Err(e) => {
                    // the admin side is not ready (no Public URL / no client id): a modal the user can read, not a passing toast
                    if !(e.message.contains("Public URL") || e.message.contains("Client ID")) { toast(&e.message, "err"); return; }
                    let uri_line = if admin {
                        format!(r#"<div class="small">Redirect URI to register at the service: <span class="mono">{}</span></div>"#, esc(&format!("{base}/api/connections/{svc}/callback")))
                    } else { String::new() };
                    modal(ModalOpts::new(&format!("Connect {}", esc(&label)), &format!(r#"<p class="muted" style="margin-top:0">{}</p>
      {uri_line}"#, esc(&e.message))).footer(r#"<button class="btn" data-close>Close</button>"#));
                }
            }
        });
        return;
    }
    if form == "ytmusic" { connect_ytmusic(d.g("available_oauth").truthy("ytmusic")); return; }
    if form == "lastfm" { connect_lastfm(); return; }
    if form == "plex" {
        dom::spawn(async move {
            let r = match api::post("/connections/plex", json!({})).await {
                Ok(r) => r,
                Err(e) => { toast(&e.message, "err"); return; }
            };
            // a popup is blocked on phones (window.open returns null) – the modal always shows the link and the progress
            let w = dom::window().open_with_url_and_target_and_features(&r.s("url"), "plexauth", "width=600,height=750").ok().flatten();
            let m = modal(ModalOpts::new("Sign in to Plex", &format!(r#"<a class="btn primary" target="_blank" rel="noopener" href="{}">{}Open Plex sign-in</a>
      <p class="small muted" style="margin-bottom:0"><span class="spinner"></span> Approve Encore in the Plex page that opens, then come back here. Waiting…</p>"#,
                esc(&r.s("url")), icon("link"))));
            let mut i = 0;
            while i < 120 && m.is_open() {
                dom::sleep(2000).await;
                if !m.is_open() { return; }
                let c = match api::post("/connections/plex/check", json!({ "pin_id": r.g("pin_id") })).await {
                    Ok(c) => c,
                    Err(e) => { m.close(); toast(&e.message, "err"); return; }
                };
                if c.truthy("done") {
                    if let Some(w) = &w { let _ = w.close(); }
                    m.close();
                    toast(&format!("Plex connected ({})", c.s("account")), "ok");
                    hashchange();
                    return;
                }
                i += 1;
            }
            if m.is_open() { m.close(); toast("Plex sign-in timed out", "err"); }
        });
        return;
    }
    let fields = match form {
        "userpass" => r#"<label class="field"><span>Username</span><input class="input" name="username" autocomplete="off"></label><label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="off"></label>"#,
        "token" => r#"<label class="field"><span>User token</span><input class="input" name="token"><small>listenbrainz.org → Settings → User token</small></label>"#,
        "username" => r#"<label class="field"><span>Last.fm username</span><input class="input" name="username"></label>"#,
        "profile" => r#"<label class="field"><span>Deezer profile link or user id</span><input class="input" name="profile" placeholder="https://www.deezer.com/profile/123456"><small>Your favourites must be public</small></label>"#,
        _ => "undefined", // `fields[undefined]` interpolated by the JS template
    };
    let m = modal(ModalOpts::new("Connect", &format!(r#"<form class="stack" id="cf">{fields}</form>"#))
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Connect</button>"#));
    if let Some(ok) = m.qs("#ok") {
        let m2 = m.clone();
        let svc = svc.to_string();
        dom::on(&ok, "click", move |_| {
            let m = m2.clone();
            let svc = svc.clone();
            dom::spawn(async move {
                let body = m.qs("#cf").map(|f| form_entries(&f)).unwrap_or_else(|| json!({}));
                match api::post(&format!("/connections/{svc}"), body).await {
                    Ok(r) => {
                        toast(&format!("Connected as {}", r.s("account")), "ok");
                        m.close();
                        hashchange();
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
    }
}

/// `connectLastfm()`: sign in on last.fm, or just a username.
fn connect_lastfm() {
    let m = modal(ModalOpts::new("Connect Last.fm", &format!(r#"<p class="muted" style="margin-top:0">Sign in on last.fm and allow Encore – then everything you play here is scrobbled, and For you suggests artists from your listening.</p>
      <button class="btn primary" id="lf-auth">{}Sign in with Last.fm</button>
      <details class="mt"><summary class="small muted" style="cursor:pointer">Or just enter a username (loved tracks and recommendations, no scrobbling)</summary>
        <div class="row mt" style="gap:8px"><input class="input grow" id="lf-user" placeholder="Last.fm username"><button class="btn" id="lf-ok">Connect</button></div></details>"#, icon("link"))));
    if let Some(b) = m.qs("#lf-auth") {
        dom::on(&b, "click", move |_| {
            dom::spawn(async move {
                match api::post("/connections/lastfm", json!({})).await {
                    Ok(r) => { let _ = dom::location().set_href(&r.s("url")); }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
    }
    if let Some(b) = m.qs("#lf-ok") {
        let m2 = m.clone();
        dom::on(&b, "click", move |_| {
            let m = m2.clone();
            dom::spawn(async move {
                let username = m.qs("#lf-user").map(|i| dom::value(&i)).unwrap_or_default().trim().to_string();
                match api::post("/connections/lastfm", json!({ "username": username })).await {
                    Ok(r) => { toast(&format!("Connected as {}", r.s("account")), "ok"); m.close(); hashchange(); }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
    }
}

/// `saveHeaders(m, headers)`: POST the pasted music.youtube.com headers and close the modal.
async fn save_headers(m: crate::ui::Modal, headers: String) {
    match api::post("/connections/ytmusic", json!({ "headers": headers })).await {
        Ok(r) => { toast(&format!("YouTube Music connected ({})", r.s("account")), "ok"); m.close(); hashchange(); }
        Err(e) => toast(&e.message, "err"),
    }
}

/// `connectYTMusic(oauth)`: device code (polled) or pasted browser headers. `oauth`: an admin added a Google OAuth
/// client, so the code sign-in can work; without one the modal opens straight on the pasted-headers path instead of
/// asking the server for a code and showing its error.
fn connect_ytmusic(oauth: bool) {
    let code_part = if oauth {
        format!(r#"<div class="tabs" id="ytm-tabs"><button data-m="code" class="active">Sign in with a code</button><button data-m="headers">Paste browser headers</button></div>
      <div id="ytm-code" class="mt"><p class="muted" style="margin-top:0">You'll get a short code to enter at google.com/device with the Google account you use for YouTube Music.</p>
        <button class="btn primary" id="ytm-start">{}Get code</button><div id="ytm-out" class="mt"></div></div>"#, icon("link"))
    } else { String::new() };
    let why = if oauth { "" } else { r#"<p class="muted" style="margin-top:0">Signing in with a code needs a Google OAuth client, which no admin has added in Settings → Integrations yet. Paste the headers of a signed-in music.youtube.com request instead – it works the same.</p>"# };
    // the textarea placeholder holds a real newline (`\n` in the JS template literal)
    let m = modal(ModalOpts::new("Connect YouTube Music", &format!(r#"{code_part}
      <div id="ytm-headers" class="{}">
        {why}
        <ol class="small muted" style="margin-top:0;line-height:1.7"><li>Open music.youtube.com in a desktop browser while signed in, press F12 → Network.</li>
          <li>Click around, find a <span class="mono">browse</span> request (POST), right-click → Copy → <b>Copy request headers</b>.</li><li>Paste below.</li></ol>
        <textarea class="input mono" id="ytm-raw" rows="8" placeholder="accept: */*
cookie: …"></textarea>
        <button class="btn primary mt" id="ytm-save">Connect</button></div>"#, if oauth { "mt hidden" } else { "mt" })).wide(true));
    if let Some(b) = m.qs("#ytm-save") {
        let m2 = m.clone();
        dom::on(&b, "click", move |_| {
            let m = m2.clone();
            let headers = m.qs("#ytm-raw").map(|t| dom::value(&t)).unwrap_or_default();
            dom::spawn(save_headers(m, headers));
        });
    }
    if !oauth { return; }
    if let Some(tabs) = m.qs("#ytm-tabs") {
        let m2 = m.clone();
        dom::on(&tabs, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-m]") else { return };
            for x in m2.qsa("#ytm-tabs button") { dom::toggle_class(&x, "active", x == b); }
            let mode = dom::data(&b, "m");
            if let Some(el) = m2.qs("#ytm-code") { dom::toggle_class(&el, "hidden", mode != "code"); }
            if let Some(el) = m2.qs("#ytm-headers") { dom::toggle_class(&el, "hidden", mode != "headers"); }
        });
    }
    if let Some(b) = m.qs("#ytm-start") {
        let m2 = m.clone();
        dom::on(&b, "click", move |ev| {
            let Some(t) = dom::target_el(ev) else { return };
            let m = m2.clone();
            dom::spawn(async move {
                dom::set_disabled(&t, true);
                let c = match api::post("/connections/ytmusic", json!({})).await {
                    Ok(c) => c,
                    Err(e) => { toast(&e.message, "err"); dom::set_disabled(&t, false); return; }
                };
                if let Some(out) = m.qs("#ytm-out") {
                    dom::set_html(&out, &format!(r#"<div class="row wrap"><span>Go to</span><a class="btn sm" target="_blank" rel="noopener" href="{url}">{url}</a><span>and enter</span>
        <span class="code" style="font-size:20px;letter-spacing:.15em">{}</span></div><p class="small muted"><span class="spinner"></span> Waiting for you to approve…</p>"#,
                        esc(&c.s("user_code")), url = esc(&c.s("url"))));
                }
                // `(c.interval || 5) * 1000`
                let interval = { let iv = c.f("interval"); if iv == 0.0 { 5.0 } else { iv } };
                let mut i = 0;
                while i < 180 && dom::body().contains(Some(m.el.as_ref())) {
                    dom::sleep((interval * 1000.0) as u32).await;
                    let r = api::post("/connections/ytmusic/check", json!({ "device_code": c.g("device_code") })).await
                        .unwrap_or_else(|_| json!({ "done": false }));
                    if r.truthy("done") {
                        toast(&format!("YouTube Music connected ({})", r.s("account")), "ok");
                        m.close();
                        hashchange();
                        return;
                    }
                    i += 1;
                }
            });
        });
    }
}

/// The `liked` table of `remotePlaylists`: where a service keeps its liked songs (deezer: null).
fn liked_url(svc: &str) -> Option<&'static str> {
    match svc {
        "spotify" => Some("https://open.spotify.com/collection/tracks"),
        "ytmusic" => Some("https://music.youtube.com/playlist?list=LM"),
        _ => None,
    }
}

/// `remotePlaylists(svc, label)`: list the playlists on the service and import them one by one.
fn remote_playlists(svc: &str, label: &str) {
    let m = modal(ModalOpts::new(&format!("Playlists on {}", esc(label)), r#"<div class="empty"><span class="spinner"></span></div>"#).wide(true));
    let svc = svc.to_string();
    dom::spawn(async move {
        let rows = match api::get(&format!("/connections/{svc}/playlists")).await {
            Ok(rows) => rows,
            Err(e) => {
                if let Some(b) = m.qs(".modal-body") { dom::set_html(&b, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); }
                return;
            }
        };
        let liked = liked_url(&svc);
        let list: String = rows.items().iter().map(|p| format!(r#"<div class="list-item" data-rid="{}" data-url="{}" data-name="{name}">
        <div class="grow ellipsis"><b>{name}</b></div>
        <label class="switch small"><input type="checkbox" data-follow checked><i></i>Follow</label>
        <button class="btn sm primary" data-import>{}Import</button></div>"#,
            esc(&p.s("id")), esc(&p.s("url")), icon("download"), name = esc(&p.s("name")))).collect();
        let list = if list.is_empty() { r#"<div class="empty muted">No playlists found</div>"#.to_string() } else { list };
        if let Some(b) = m.qs(".modal-body") {
            dom::set_html(&b, &format!(r#"
      {}
      <div class="card flat"><div class="list">{list}</div></div>"#,
                if liked.is_some() { r#"<button class="btn mt" id="liked" style="margin:0 0 12px">♥ Import Liked Songs</button>"# } else { "" }));
        }
        if let (Some(b), Some(url)) = (m.qs("#liked"), liked) {
            let m2 = m.clone();
            dom::on(&b, "click", move |_| { m2.close(); import_modal(url, None); });
        }
        if let Some(list_el) = m.qs(".list") {
            dom::on(&list_el, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-import]") else { return };
                let Some(row) = dom::closest_of(&b, "[data-rid]") else { return };
                let svc = svc.clone();
                dom::spawn(async move {
                    dom::set_disabled(&b, true);
                    let name = dom::data(&row, "name");
                    let url = dom::data(&row, "url");
                    let url = if url.is_empty() { Value::Null } else { Value::String(url) };
                    let rid = String::from(js_sys::encode_uri_component(&dom::data(&row, "rid")));
                    let body = json!({ "name": name, "url": url, "follow": dom::checked_of(&row, "[data-follow]") });
                    match api::post(&format!("/connections/{svc}/playlists/{rid}/import"), body).await {
                        Ok(r) => {
                            let dup = r.truthy("duplicates_skipped");
                            toast(&format!("Imported {name}: {} tracks{}", r.s("added"),
                                if dup { format!(", {} duplicates skipped", r.s("duplicates_skipped")) } else { String::new() }), "ok");
                            b.set_outer_html(r#"<span class="badge ok">imported</span>"#);
                        }
                        Err(err) => { toast(&err.message, "err"); dom::set_disabled(&b, false); }
                    }
                });
            });
        }
    });
}
