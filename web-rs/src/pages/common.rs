//! Port of web/js/pages/common.js – see docs/ui-rewrite/CONTRACT.md
//! What many pages share: sending a request, album and library tiles, request rows with their
//! action buttons, and the "Clear" menu of the download lists. Same HTML, same API calls.

use crate::json::{js_string, json, truthy, J};
use crate::ui::{self, esc, esck, icon, status_badge, toast, MenuItem};
use crate::{api, app, dom, pages, player};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;

/// `submitRequest(body, btn)`: POST the request, toast the outcome and swap the button for the status badge.
pub async fn submit_request(body: Value, btn: Option<Element>) -> Option<Value> {
    if let Some(b) = &btn { dom::set_disabled(b, true); }
    match api::post("/requests", body).await {
        Ok(r) => {
            toast(if r.s("status") == "pending" { "Request sent – waiting for approval" } else { "Request queued" }, "ok");
            dom::dispatch("encore:requests-changed", None);
            if let Some(b) = &btn {
                dom::closest_of(b, ".req-group").unwrap_or_else(|| b.clone()).set_outer_html(&status_badge(&r.s("status")));
            }
            Some(r)
        }
        Err(e) => {
            toast(&e.message, "err");
            if let Some(b) = &btn { dom::set_disabled(b, false); }
            None
        }
    }
}

/// `a.x || a.y`, as a string.
fn or(v: &Value, a: &str, b: &str) -> String { if v.truthy(a) { v.s(a) } else { v.s(b) } }

/// `albumTile(a, { href, actions, sub })`.
pub fn album_tile(a: &Value, href: &str, actions: &str, sub: Option<&str>) -> String {
    album_tile_with(a, href, actions, sub, None, None)
}

/// `albumTile(a, { href, actions, sub, key, badge })`: `key` marks the corner badge for live request
/// labels (see discover.rs), `badge` replaces its contents.
pub fn album_tile_with(a: &Value, href: &str, actions: &str, sub: Option<&str>, key: Option<&str>, badge: Option<&str>) -> String {
    let corner = match badge {
        Some(b) => b.to_string(),
        None if a.truthy("in_library") => r#"<span class="badge ok">In library</span>"#.to_string(),
        None if a.truthy("request_status") => status_badge(&a.s("request_status")),
        None => String::new(),
    };
    let key_attr = key.filter(|k| !k.is_empty()).map(|k| format!(r#" data-req-key="{}""#, esc(k))).unwrap_or_default();
    let sub = match sub {
        Some(s) => s.to_string(),
        None => {
            let artist = ui::tr_inline(a, "artist");
            let rest = ["year", "type"].iter().map(|k| a.g(k)).filter(|v| truthy(v)).map(js_string);
            esc(&std::iter::once(artist).filter(|x| !x.is_empty()).chain(rest).collect::<Vec<_>>().join(" · "))
        }
    };
    format!(r#"<div class="tile" data-href="{href}">
    <div class="art">{art}
      <div class="corner"{key_attr}>{corner}</div>
      <div class="overlay">{actions}</div>
    </div>
    <div class="t ellipsis" title="{title}">{shown}</div>{under}
    <div class="s ellipsis">{sub}</div>
  </div>"#,
        href = esc(href),
        art = ui::art(&a.s("cover"), &a.s("title"), ""),
        title = esck(a, "title"), shown = esc(&ui::tr(a, "title")), under = ui::tr_sub(a, "title"))
}

/// `libraryTile(a)`.
pub fn library_tile(a: &Value) -> String {
    let id = a.s("id");
    format!(r#"<div class="tile" data-href="/library/album/{id}">
    <div class="art">{art}
      {untagged}
      <div class="overlay"><button class="fab" data-play-album="{id}" aria-label="Play">{play}</button></div>
    </div>
    <div class="t ellipsis" title="{album}">{album_or}</div>{under}
    <div class="s ellipsis">{artist}{year}</div>
  </div>"#,
        id = esc(&id),
        art = ui::art(&format!("/api/library/albums/{}/cover{}", a.s("id"), ui::cover_q(a)), &a.s("album"), ""),
        untagged = if a.truthy("untagged") { r#"<div class="corner"><span class="badge warn" title="Some tracks lack MusicBrainz tags">Untagged</span></div>"# } else { "" },
        play = icon("play"),
        album = esck(a, "album"),
        album_or = if a.truthy("album") { esc(&ui::tr(a, "album")) } else { "Unknown album".to_string() },
        under = ui::tr_sub(a, "album"),
        artist = esc(&ui::tr_inline(a, "albumartist")),
        year = if a.truthy("year") { format!(" · {}", esck(a, "year")) } else { String::new() })
}

/// `bindTiles(root, navigate)`: the play buttons on tiles and the tile itself (navigates with `app::navigate`).
pub fn bind_tiles(root: &Element) {
    dom::on(root, "click", |e| {
        if let Some(play) = dom::closest(e, "[data-play-album]") {
            dom::stop(e);
            pages::library::play_album(&dom::data(&play, "playAlbum"), 0);
            return;
        }
        if let Some(song) = dom::closest(e, "[data-play-song]") {
            dom::stop(e);
            let Ok(t) = serde_json::from_str::<Value>(&dom::data(&song, "playSong")) else { return };
            player::play_queue(vec![t], 0, "list");
            return;
        }
        if dom::closest(e, "button, a, input").is_some() { return; }
        if let Some(tile) = dom::closest(e, "[data-href]") { app::navigate(&dom::data(&tile, "href")); }
    });
}

/// `JSON.stringify(v)` with numbers printed the JavaScript way.
fn json_text(v: &Value) -> String {
    match v {
        Value::Number(_) => js_string(v),
        Value::Array(a) => format!("[{}]", a.iter().map(json_text).collect::<Vec<_>>().join(",")),
        Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}:{}", Value::String(k.clone()), json_text(v))).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// The `data-req` of the Pick button: `JSON.stringify({ request_id: r.id, kind, mbid, artist, title, album, length })`
/// – same key order, and fields the request lacks are left out like `JSON.stringify` does with `undefined`.
fn pick_json(r: &Value) -> String {
    let fields = [("request_id", "id"), ("kind", "kind"), ("mbid", "mbid"), ("artist", "artist"), ("title", "title"), ("album", "album"), ("length", "length")];
    let body: Vec<String> = fields.iter().filter(|(_, k)| r.has(k)).map(|(name, k)| format!("\"{name}\":{}", json_text(r.g(k)))).collect();
    format!("{{{}}}", body.join(","))
}

/// `requestRow(r, { admin })`.
pub fn request_row(r: &Value, admin: bool) -> String {
    let kind = r.s("kind");
    let status = r.s("status");
    let what = match kind.as_str() {
        "album" => format!(r#"{} <span class="muted">· {}</span>"#, esc(&or(r, "title", "album")), esck(r, "artist")),
        "artist" => format!(r#"{} <span class="muted">· full discography</span>"#, esck(r, "artist")),
        "url" => esc(&or(r, "title", "url")),
        "playlist" => format!(r#"{} <span class="muted">· playlist import</span>"#, esck(r, "title")),
        _ => format!(r#"{} <span class="muted">· {}</span>"#, esck(r, "title"), esck(r, "artist")),
    };
    let active = matches!(status.as_str(), "searching" | "downloading" | "importing" | "monitoring");
    format!(r#"<div class="list-item" data-id="{id}">
    <div class="thumb">{thumb}</div>
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">{what}</div>
      <div class="small muted ellipsis">
        <span class="badge" style="padding:0 7px">{kind}</span>
        {source}
        {message}
        {by}
        · <span class="faint">{ago}</span>
      </div>
      {progress}
    </div>
    <div class="row">
      {badge}
      {approve}
      {pick}
      {retry}
      {cancel}
      {menu}
    </div>
  </div>"#,
        id = r.s("id"),
        thumb = ui::art(&r.s("cover_url"), &or(r, "title", "artist"), ""),
        kind = esc(&kind),
        source = if r.truthy("source") { format!(r#"<span class="faint">via {}</span> ·"#, esck(r, "source")) } else { String::new() },
        message = if r.truthy("message") { esck(r, "message") } else { String::new() },
        by = if admin && r.truthy("username") { format!(r#" · <span class="faint">by {}</span>"#, esck(r, "username")) } else { String::new() },
        ago = ui::fmt::ago(r.f("created_at")),
        progress = if active && r.truthy("progress") { format!(r#"<div class="progress" style="margin-top:6px;max-width:420px"><i style="width:{}%"></i></div>"#, r.s("progress")) } else { String::new() },
        badge = status_badge(&status),
        approve = if admin && status == "pending" { format!(r#"<button class="btn sm primary" data-act="approve">{}Approve</button><button class="btn sm danger" data-act="deny">Deny</button>"#, icon("check")) } else { String::new() },
        pick = if matches!(kind.as_str(), "album" | "track") && status != "completed" { format!(r#"<button class="btn sm" data-act="pick" data-req='{}' title="Search the sources and pick the exact download">{}Pick</button>"#, esc(&pick_json(r)), icon("search")) } else { String::new() },
        retry = if matches!(status.as_str(), "failed" | "cancelled") || kind == "playlist" { format!(r#"<button class="btn sm" data-act="retry" title="Retry">{}</button>"#, icon("refresh")) } else { String::new() },
        cancel = if matches!(status.as_str(), "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing") { format!(r#"<button class="btn sm" data-act="cancel" title="Cancel">{}</button>"#, icon("x")) } else { String::new() },
        menu = if !active { format!(r#"<button class="btn sm ghost icon" data-act="menu" data-status="{}" aria-label="More">{}</button>"#, esc(&status), icon("more")) } else { String::new() })
}

/// `bindRequestActions(root, reload)`: approve / deny / retry / cancel / pick and the ⋯ menu of request rows.
pub fn bind_request_actions(root: &Element, reload: Rc<dyn Fn()>) {
    dom::on(root, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-act]") else { return };
        let Some(row) = dom::closest_of(&b, "[data-id]") else { return };
        let id = dom::data(&row, "id");
        let act = dom::data(&b, "act");
        if act == "menu" {
            let st = dom::data(&b, "status");
            let del: Rc<dyn Fn(bool)> = {
                let (id, st, reload) = (id.clone(), st.clone(), reload.clone());
                Rc::new(move |files: bool| {
                    let (id, st, reload) = (id.clone(), st.clone(), reload.clone());
                    dom::spawn(async move {
                        if files && !ui::confirm_dialog("Delete files", if st == "completed" { "Permanently delete the songs this request downloaded from the library?" } else { "Delete the leftover/partial files of this download?" }, "Delete", true).await { return; }
                        match api::del(&format!("/requests/{id}{}", if files { "?files=true" } else { "" })).await {
                            Ok(r) => {
                                if files { toast(&format!("{} file(s) deleted", r.s("deleted_files")), "ok"); }
                                dom::dispatch("encore:requests-changed", None);
                                reload();
                            }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                })
            };
            let mut items = vec![MenuItem::new("Remove from list", &icon("x"), { let del = del.clone(); move || del(false) })];
            if app::is_admin() {
                items.push(MenuItem::danger(if st == "completed" { "Delete downloaded files" } else { "Delete leftover files" }, &icon("trash"), move || del(true)));
            }
            ui::pop_menu(&b, items);
            return;
        }
        if act == "pick" {
            let Ok(opts) = serde_json::from_str::<Value>(&dom::data(&b, "req")) else { return };
            // `onDone: reload` – the list refreshes after a pick
            let r = reload.clone();
            pages::picker::picker_modal_with(opts, Some(Rc::new(move |_| r())));
            return;
        }
        dom::set_disabled(&b, true);
        let reload = reload.clone();
        dom::spawn(async move {
            let res = if act == "delete" { api::del(&format!("/requests/{id}")).await } else { api::post_empty(&format!("/requests/{id}/{act}")).await };
            match res {
                Ok(_) => {
                    dom::dispatch("encore:requests-changed", None);
                    reload();
                }
                Err(err) => {
                    toast(&err.message, "err");
                    dom::set_disabled(&b, false);
                }
            }
        });
    });
}

/// `clearMenu(anchor, reload, { mine })`: "Clear" button menu for download lists – clear finished entries,
/// optionally deleting their files.
pub fn clear_menu(anchor: &Element, reload: Rc<dyn Fn()>, mine: bool) {
    let admin = app::is_admin();
    let go = Rc::new(move |statuses: &'static [&'static str], files: bool, label: &'static str| {
        let reload = reload.clone();
        dom::spawn(async move {
            if files && !ui::confirm_dialog("Delete files", &format!("{label}: this permanently deletes the files from disk."), "Delete", true).await { return; }
            match api::post("/requests/clear", json!({ "statuses": statuses, "files": files, "mine": mine })).await {
                Ok(r) => {
                    toast(&format!("{} cleared{}", r.s("removed"), if files { format!(" · {} file(s) deleted", r.s("deleted_files")) } else { String::new() }), "ok");
                    dom::dispatch("encore:requests-changed", None);
                    reload();
                }
                Err(e) => toast(&e.message, "err"),
            }
        });
    });
    let mut items = vec![
        MenuItem::new("Clear completed", &icon("check"), { let go = go.clone(); move || go(&["completed"], false, "") }),
        MenuItem::new("Clear failed & cancelled", &icon("x"), { let go = go.clone(); move || go(&["failed", "cancelled", "denied"], false, "") }),
    ];
    if admin {
        items.push(MenuItem::SEP);
        items.push(MenuItem::danger("Clear failed & cancelled + delete their files", &icon("trash"), { let go = go.clone(); move || go(&["failed", "cancelled", "denied"], true, "Failed & cancelled") }));
        items.push(MenuItem::danger("Clear completed + delete the songs", &icon("trash"), move || go(&["completed"], true, "Completed downloads")));
    }
    ui::pop_menu(anchor, items);
}
