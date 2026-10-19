//! Port of web/js/pages/common.js – see docs/archive/ui-rewrite/CONTRACT.md
//! What many pages share: sending a request, album and library tiles, request rows with their
//! action buttons, and the "Clear" menu of the download lists. Same HTML, same API calls.

use crate::json::{js_string, json, truthy, J};
use crate::ui::{self, esc, esck, icon, status_badge, toast, MenuItem};
use crate::{api, app, details, dom, pages, player};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
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

/// `bindTiles(root, navigate)`: the play buttons on tiles and the tile itself (navigates with `app::navigate`), and
/// the ⋯ of library song rows (`data-song-menu`, the song as JSON).
pub fn bind_tiles(root: &Element) {
    dom::on(root, "click", |e| {
        if let Some(play) = dom::closest(e, "[data-play-album]") {
            dom::stop(e);
            pages::library::play_album(&dom::data(&play, "playAlbum"), 0);
            return;
        }
        if let Some(b) = dom::closest(e, "[data-song-menu]") {
            dom::stop(e);
            let Ok(t) = serde_json::from_str::<Value>(&dom::data(&b, "songMenu")) else { return };
            pages::library::track_menu(&b, &t, app::is_admin(), None);
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

thread_local! {
    /// The request rows as the lists last drew them, by id: what Details shows and copies.
    static ROWS: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
}

/// The row a list drew for this request id.
fn drawn_row(id: &str) -> Option<Value> { ROWS.with(|m| m.borrow().get(id).cloned()) }

/// Kept downloads (`has_files`): the files of a failed or waiting import (or the files a link's request could not
/// import) are still there, so the row offers Import again where it offered Retry – nothing is downloaded again.
pub fn can_import_again(r: &Value) -> bool {
    r.truthy("has_files") && matches!(r.s("status").as_str(), "failed" | "waiting" | "completed")
}

/// What Cancel and Remove from list say first when a request keeps downloaded files: they go with it (Encore's own
/// staging folder is deleted) and Import again is gone.
pub const KEPT_FILES_LOST: &str = "Its downloaded files have not been imported yet and can't be imported after this.";

/// `requestRow(r, { admin })`.
pub fn request_row(r: &Value, admin: bool) -> String {
    ROWS.with(|m| m.borrow_mut().insert(r.s("id"), r.clone()));
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
    format!(r#"<div class="list-item req-row" data-id="{id}">
    <div class="thumb">{thumb}</div>
    <div class="grow req-text" data-act="details" title="Show the whole text">
      <div class="ellipsis" style="font-weight:600">{what}</div>
      <div class="small muted ellipsis"{full}>
        <span class="badge" style="padding:0 7px">{kind}</span>
        {attempts}
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
        attempts = { let n = r.f("attempts") as i64; if n > 1 { format!(r#"<span class="faint" title="Requested or retried {n} times">{n} attempts</span> ·"#) } else { String::new() } },
        source = if r.truthy("source") { format!(r#"<span class="faint">via {}</span> ·"#, esck(r, "source")) } else { String::new() },
        message = if r.truthy("message") { esck(r, "message") } else { String::new() },
        full = if r.truthy("message") { format!(r#" title="{}""#, esck(r, "message")) } else { String::new() },
        by = if admin && r.truthy("username") { format!(r#" · <span class="faint">by {}</span>"#, esck(r, "username")) } else { String::new() },
        ago = ui::fmt::ago(r.f("created_at")),
        progress = if active && r.truthy("progress") { format!(r#"<div class="progress" style="margin-top:6px;max-width:420px"><i style="width:{}%"></i></div>"#, r.s("progress")) } else { String::new() },
        badge = status_badge(&status),
        approve = if admin && status == "pending" { format!(r#"<button class="btn sm primary" data-act="approve">{}Approve</button><button class="btn sm danger" data-act="deny">Deny</button>"#, icon("check")) } else { String::new() },
        pick = if matches!(kind.as_str(), "album" | "track") && status != "completed" { format!(r#"<button class="btn sm" data-act="pick" data-req='{}' title="Search the sources and pick the exact download">{}Pick</button>"#, esc(&pick_json(r)), icon("search")) } else { String::new() },
        // a failed or waiting import whose files are kept: Import again takes Retry's place
        retry = if can_import_again(r) { format!(r#"<button class="btn sm" data-act="import-again" title="Import the downloaded files again – nothing is downloaded again">{}Import again</button>"#, icon("refresh")) }
            else if matches!(status.as_str(), "failed" | "cancelled") || kind == "playlist" { format!(r#"<button class="btn sm" data-act="retry" title="Retry">{}</button>"#, icon("refresh")) } else { String::new() },
        cancel = if matches!(status.as_str(), "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing" | "waiting") { format!(r#"<button class="btn sm" data-act="cancel" title="Cancel">{}</button>"#, icon("x")) } else { String::new() },
        menu = if !active { format!(r#"<button class="btn sm ghost icon" data-act="menu" data-status="{}"{} aria-label="More">{}</button>"#, esc(&status), if r.truthy("has_files") { r#" data-files="1""# } else { "" }, icon("more")) } else { String::new() })
}

/// `bindRequestActions(root, reload)`: approve / deny / retry / cancel / pick and the ⋯ menu of request rows.
pub fn bind_request_actions(root: &Element, reload: Rc<dyn Fn()>) {
    dom::on(root, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-act]") else { return };
        let Some(row) = dom::closest_of(&b, "[data-id]") else { return };
        let id = dom::data(&row, "id");
        let act = dom::data(&b, "act");
        if act == "details" {
            // a click that ends a text selection on the row is not a request for the dialog
            if dom::closest(e, "a, button").is_some() || details::selecting() { return; }
            show_request_details(&id);
            return;
        }
        if act == "menu" {
            let st = dom::data(&b, "status");
            let kept = dom::data(&b, "files") == "1";
            let del: Rc<dyn Fn(bool)> = {
                let (id, st, reload) = (id.clone(), st.clone(), reload.clone());
                Rc::new(move |files: bool| {
                    let (id, st, reload) = (id.clone(), st.clone(), reload.clone());
                    dom::spawn(async move {
                        let ask = if st == "completed" { "Permanently delete the songs this request downloaded from the library?" }
                            else if kept { "Delete the downloaded files kept for this request? They can't be imported afterwards." }
                            else { "Delete the leftover/partial files of this download?" };
                        if files && !ui::confirm_dialog("Delete files", ask, "Delete", true).await { return; }
                        // the files kept for Import again go with the request
                        if !files && kept && !ui::confirm_dialog("Remove this request?", KEPT_FILES_LOST, "Remove", true).await { return; }
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
            let id2 = id.clone();
            let mut items = vec![
                MenuItem::new("Details", &icon("list"), move || show_request_details(&id2)),
                MenuItem::new("Remove from list", &icon("x"), { let del = del.clone(); move || del(false) }),
            ];
            if app::is_admin() {
                items.push(MenuItem::danger(if st == "completed" || kept { "Delete downloaded files" } else { "Delete leftover files" }, &icon("trash"), move || del(true)));
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
        // Cancel on a request that keeps downloaded files: they go with it, so it asks first
        let kept = act == "cancel" && drawn_row(&id).is_some_and(|r| r.truthy("has_files"));
        dom::spawn(async move {
            if kept && !ui::confirm_dialog("Cancel this request?", KEPT_FILES_LOST, "Cancel request", true).await {
                dom::set_disabled(&b, false);
                return;
            }
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

/// Details of a request row: everything the row cuts off, in a dialog with Copy.
fn show_request_details(id: &str) {
    let Some(r) = drawn_row(id) else { return };
    let text = request_text(&r, &app::info().s("version"), &ui::fmt::date);
    let what = request_what(&r);
    details::show(if what.is_empty() { "Details" } else { &what }, &text);
}

/// What a request asked for, in one line: "Illuminate EP – Duumu", "Duumu – full discography", the link.
fn request_what(r: &Value) -> String {
    let title = match r.s("kind").as_str() {
        "album" => or(r, "title", "album"),
        "url" => or(r, "title", "url"),
        "artist" => String::new(),
        _ => r.s("title"),
    };
    let artist = r.s("artist");
    match (title.is_empty(), artist.is_empty()) {
        (false, false) => format!("{title} – {artist}"),
        (false, true) => title,
        (true, false) if r.s("kind") == "artist" => format!("{artist} – full discography"),
        (true, false) => artist,
        _ => String::new(),
    }
}

/// The words for what a waiting request waits for (`wait_reason`).
pub fn wait_words(reason: &str) -> &'static str {
    match reason {
        "soulseek" => "Soulseek isn't connected yet",
        "storage_import" => "the downloaded files are kept and the import waits for the storage",
        "storage_download" => "the download waits for the staging folder",
        _ => "",
    }
}

/// A request in plain text, for the Details dialog and its Copy – what a bug report needs: title, artist, status
/// and message, every attempt in the log with its source and whole error, the failed step with its operation,
/// path and errno, streamrip's output, the request id and the server's version. `date` formats a unix time
/// (`ui::fmt::date` on the page).
pub fn request_text(r: &Value, version: &str, date: &dyn Fn(f64) -> String) -> String {
    let mut out: Vec<String> = vec![format!("Encore request #{}", r.s("id"))];
    let mut add = |k: &str, v: String| { if !v.trim().is_empty() { out.push(format!("{k}: {v}")); } };
    let kind = r.s("kind");
    let title = match kind.as_str() { "album" => or(r, "title", "album"), "url" => r.s("title"), _ => r.s("title") };
    add("Title", title);
    add("Artist", r.s("artist"));
    if kind == "track" && r.s("album") != r.s("title") { add("Album", r.s("album")); }
    add("Link", r.s("url"));
    add("Kind", kind.clone());
    let status = r.s("status");
    let label = ui::status_info(&status).1;
    let why = if status == "waiting" { wait_words(&r.s("wait_reason")) } else { "" };
    add("Status", if why.is_empty() { label } else { format!("{label} – {why}") });
    add("Message", r.s("message"));
    if status == "waiting" && r.f("next_try_at") > 0.0 { add("Next try", date(r.f("next_try_at"))); }
    add("Attempts", r.s("attempts"));
    add("Source", r.s("source"));
    if r.truthy("picked") { add("Picked by hand", "yes".into()); }
    let c = r.g("candidate");
    if c.is_object() && (c.truthy("username") || c.truthy("directory")) {
        let extra: Vec<String> = [c.s("format").to_uppercase(), if c.i("files") > 0 { format!("{} file{}", c.i("files"), if c.i("files") == 1 { "" } else { "s" }) } else { String::new() }]
            .into_iter().filter(|x| !x.is_empty()).collect();
        let what = format!("{}{}", c.s("directory"), if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) });
        add("Downloading", if c.truthy("username") { format!("from {}: {what}", c.s("username")) } else { what });
    }
    let e = r.g("error");
    if e.is_object() {
        add("Error", if e.truthy("text") { e.s("text") } else { e.s("error") });
        add("Step", e.s("step"));
        add("Operation", e.s("op"));
        add("Path", e.s("path"));
        add("Other path", e.s("to"));
        add("Errno", e.s("errno"));
        if e.truthy("text") { add("System error", e.s("error")); }
        if e.truthy("storage") { add("Storage problem", "yes – not the source".into()); }
    }
    // (other users than admins get no server folder: the files are on the server, in staging or a download client's folder)
    if r.truthy("has_files") { add("Downloaded files kept in", if r.truthy("kept_path") { r.s("kept_path") } else { "the server".into() }); }
    let log: Vec<String> = r.a("log").iter().map(crate::json::js_string).filter(|l| !l.trim().is_empty()).collect();
    if !log.is_empty() {
        out.push(String::new());
        out.push("Log (every attempt, oldest first):".into());
        out.extend(log);
    }
    let o = r.g("output");
    if o.is_object() && o.truthy("text") {
        let head: Vec<String> = [o.s("url"), (if o.truthy("ok") { "produced files" } else { "failed" }).to_string(), if o.f("at") > 0.0 { date(o.f("at")) } else { String::new() }]
            .into_iter().filter(|x| !x.is_empty()).collect();
        out.push(String::new());
        out.push(format!("{} output ({}):", if o.truthy("source") { o.s("source") } else { "streamrip".into() }, head.join(" · ")));
        out.push(o.s("text").trim_end().to_string());
    }
    out.push(String::new());
    let mut add = |k: &str, v: String| { if !v.trim().is_empty() { out.push(format!("{k}: {v}")); } };
    add("Requested by", r.s("username"));
    if r.f("created_at") > 0.0 { add("Requested", date(r.f("created_at"))); }
    if r.f("updated_at") > 0.0 { add("Updated", date(r.f("updated_at"))); }
    add("Request id", r.s("id"));
    add("Server version", if version.is_empty() { "unknown".into() } else { version.to_string() });
    out.join("\n")
}

// ---- the storage banner (Requests, Downloader): imports paused because a folder can't be written ----------

/// The banner for `GET /api/requests/storage`: nothing while imports go on. Admins read the folder, the operation
/// and the error (the server's sentence, with its clock) and get Check again; everyone else the plain words.
pub fn storage_banner_html(s: &Value, admin: bool) -> String {
    if !s.is_object() || s.g("ok") != &Value::Bool(false) { return String::new(); }
    let text = if s.truthy("text") { s.s("text") }
        else if admin && s.truthy("path") {
            format!("Imports are paused: {} is not writable – {}{}.", s.s("path"), if s.truthy("op") { format!("{}: ", s.s("op")) } else { String::new() }, s.s("error"))
        } else { "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed.".to_string() };
    let check = if admin { format!(r#"<button class="btn sm" data-storage-check>{}Check again</button>"#, icon("refresh")) } else { String::new() };
    format!(r#"<div class="storage-banner" role="status">{}<span>{}</span>{check}</div>"#, icon("alert"), esc(&text))
}

/// Draw the storage banner into `slot` now and every 30 s while the page is open (every 5 s while imports
/// are paused, so it goes as soon as an import or the 10-minute check finds the folder writable again, not up
/// to 30 s after the request shows Available); Check again probes at once.
pub fn storage_banner(view: &Element, slot: &Element, admin: bool) {
    let paused = Rc::new(std::cell::Cell::new(false));
    let paint = {
        let (slot, paused) = (slot.clone(), paused.clone());
        Rc::new(move |s: &Value| {
            paused.set(s.g("ok") == &Value::Bool(false));
            dom::set_html(&slot, &storage_banner_html(s, admin))
        })
    };
    let load: Rc<dyn Fn()> = {
        let paint = paint.clone();
        // an older server has no /requests/storage: no banner
        Rc::new(move || { let paint = paint.clone(); dom::spawn(async move { if let Ok(s) = api::get("/requests/storage").await { paint(&s); } }); })
    };
    load();
    let ticks = std::cell::Cell::new(0u32);
    dom::interval(view, 5_000, {
        let load = load.clone();
        move || {
            ticks.set(ticks.get().wrapping_add(1));
            if paused.get() || ticks.get().is_multiple_of(6) { load() }
        }
    });
    dom::on(slot, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-storage-check]") else { return };
        dom::set_disabled(&b, true);
        dom::set_html(&b, r#"<span class="spinner" style="width:12px;height:12px"></span> Checking…"#);
        let paint = paint.clone();
        dom::spawn(async move {
            match api::post_empty("/requests/storage/check").await {
                Ok(s) => {
                    paint(&s);
                    if s.g("ok") == &Value::Bool(false) { toast("Imports are still paused", "warn"); }
                    else { toast("Imports resumed", "ok"); dom::dispatch("encore:requests-changed", None); }
                }
                Err(err) => {
                    toast(&err.message, "err");
                    dom::set_disabled(&b, false);
                    dom::set_html(&b, &format!("{}Check again", icon("refresh")));
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

/// Retry failed: queue every failed request again (admins: everyone's, unless `mine`), say how many, reload.
pub fn retry_failed(mine: bool, reload: Rc<dyn Fn()>) {
    dom::spawn(async move {
        match api::post("/requests/retry-failed", serde_json::json!({ "mine": mine })).await {
            Ok(r) => {
                let (n, skipped) = (r.g("retried").as_i64().unwrap_or(0), r.g("skipped").as_i64().unwrap_or(0));
                let msg = match (n, skipped) {
                    (0, 0) => "No failed requests to retry".to_string(),
                    (n, 0) => format!("Retrying {n} request{}", if n == 1 { "" } else { "s" }),
                    (n, s) => format!("Retrying {n} · {s} couldn't be retried{}", r.g("why").as_str().map(|w| format!(" ({w})")).unwrap_or_default()),
                };
                toast(&msg, if skipped > 0 { "warn" } else { "ok" });
                reload();
            }
            Err(e) => toast(&e.message, "err"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A row of GET /api/requests whose import waits for the storage (S1's answer, R1).
    fn waiting_row() -> Value {
        json!({ "id": 12, "kind": "album", "status": "waiting", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP",
            "source": "streamrip", "username": "caz", "created_at": 0, "attempts": 3, "picked": true,
            "message": "Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20",
            "wait_reason": "storage_import", "next_try_at": 1792427497.2, "has_files": true, "kept_path": "/downloads/encore/request-12/streamrip",
            "error": { "step": "import", "op": "mkdir", "path": "/music/Duumu/Illuminate EP", "to": null, "errno": 5,
                "error": "Input/output error (os error 5)", "storage": true,
                "text": "import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source" },
            "output": { "source": "streamrip", "url": "https://www.deezer.com/album/1", "text": "Downloading Illuminate EP\n4 tracks done\n", "ok": true, "at": 1792427000.1 },
            "log": ["slskd: error: Soulseek isn't connected yet", "attempt 2 failed: streamrip: Input/output error (os error 5)"],
            "candidate": null })
    }

    fn date(t: f64) -> String { format!("<{}>", t as i64) }

    #[test]
    fn a_waiting_import_reads_waiting_and_offers_import_again() {
        let r = waiting_row();
        let html = request_row(&r, true);
        assert!(html.contains(r#"<span class="badge warn">Waiting</span>"#), "{html}");
        assert!(html.contains(r#"data-act="import-again""#) && html.contains("Import again</button>"), "{html}");
        assert!(!html.contains(r#"data-act="retry""#), "Import again takes Retry's place: {html}");
        // it can be cancelled, its ⋯ knows the files are kept, and the cut-off line opens the whole text
        assert!(html.contains(r#"data-act="cancel""#), "{html}");
        assert!(html.contains(r#"data-status="waiting" data-files="1""#), "{html}");
        assert!(html.contains(r#"class="grow req-text" data-act="details""#), "{html}");
        assert!(html.contains(r#"title="Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20""#), "{html}");
        // no spinner and no progress: nothing runs while it waits
        assert!(!html.contains("spinner") && !html.contains(r#"class="progress""#), "{html}");
        assert_eq!(drawn_row("12").map(|v| v.s("title")), Some("Illuminate EP".to_string()));

        // a failed import whose files are kept: Import again too
        let mut f = r.clone();
        f["status"] = json!("failed");
        f["wait_reason"] = Value::Null;
        let html = request_row(&f, false);
        assert!(html.contains("Import again</button>") && !html.contains(r#"data-act="retry""#), "{html}");
        // a plain failure keeps Retry; a Soulseek wait has no files and no Import again
        f["has_files"] = json!(false);
        let html = request_row(&f, false);
        assert!(html.contains(r#"data-act="retry""#) && !html.contains("Import again"), "{html}");
        let s = json!({ "id": 13, "kind": "track", "status": "waiting", "title": "Blow", "artist": "Ke$ha", "created_at": 0, "attempts": 1,
            "message": "Waiting for Soulseek – next attempt at 14:20", "wait_reason": "soulseek", "has_files": false });
        let html = request_row(&s, false);
        assert!(html.contains(r#"<span class="badge warn">Waiting</span>"#) && !html.contains("Import again") && html.contains(r#"data-act="cancel""#), "{html}");
        assert_eq!(wait_words("soulseek"), "Soulseek isn't connected yet");
        // a link's request that kept the files it could not import offers Import again for them
        let c = json!({ "id": 14, "kind": "url", "status": "completed", "title": "Mix", "created_at": 0, "has_files": true,
            "message": "Downloaded 1 of 2 file(s) – the rest could not be imported: …" });
        assert!(can_import_again(&c) && request_row(&c, true).contains("Import again</button>"));
        assert!(!can_import_again(&json!({ "status": "completed", "has_files": false })));
    }

    #[test]
    fn the_details_hold_what_a_bug_report_needs() {
        let t = request_text(&waiting_row(), "0.29.0", &date);
        let want = "Encore request #12
Title: Illuminate EP
Artist: Duumu
Kind: album
Status: Waiting – the downloaded files are kept and the import waits for the storage
Message: Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20
Next try: <1792427497>
Attempts: 3
Source: streamrip
Picked by hand: yes
Error: import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source
Step: import
Operation: mkdir
Path: /music/Duumu/Illuminate EP
Errno: 5
System error: Input/output error (os error 5)
Storage problem: yes – not the source
Downloaded files kept in: /downloads/encore/request-12/streamrip

Log (every attempt, oldest first):
slskd: error: Soulseek isn't connected yet
attempt 2 failed: streamrip: Input/output error (os error 5)

streamrip output (https://www.deezer.com/album/1 · produced files · <1792427000>):
Downloading Illuminate EP
4 tracks done

Requested by: caz
Request id: 12
Server version: 0.29.0";
        assert_eq!(t, want);
        // a link that failed with nothing kept: no empty lines for what it doesn't have
        let r = json!({ "id": 7, "kind": "url", "status": "failed", "url": "https://www.youtube.com/watch?v=x", "title": "", "created_at": 1700000000,
            "message": "Downloaded, but nothing could be imported: …", "attempts": 1, "error": null, "output": null, "log": [], "has_files": false });
        let t = request_text(&r, "", &date);
        assert_eq!(t, "Encore request #7\nLink: https://www.youtube.com/watch?v=x\nKind: url\nStatus: Failed\nMessage: Downloaded, but nothing could be imported: …\nAttempts: 1\n\nRequested: <1700000000>\nRequest id: 7\nServer version: unknown");
        assert_eq!(request_what(&waiting_row()), "Illuminate EP – Duumu");
        assert_eq!(request_what(&r), "https://www.youtube.com/watch?v=x");
        assert_eq!(request_what(&json!({ "kind": "artist", "artist": "Duumu" })), "Duumu – full discography");
    }

    #[test]
    fn the_storage_banner_for_admins_and_members() {
        let admin = json!({ "ok": false, "since": 1792426891.6, "checked_at": 1792426891.6, "next_check_at": 1792427491.6, "path": "/music", "op": "mkdir",
            "error": "Input/output error (os error 5)", "errno": 5, "step": "import", "waiting": 3,
            "text": "Imports are paused: /music is not writable – mkdir /music/.encore-probe-12-3: Input/output error (os error 5). Checked 14:10, next check 14:20" });
        let html = storage_banner_html(&admin, true);
        assert!(html.contains("<span>Imports are paused: /music is not writable – mkdir /music/.encore-probe-12-3: Input/output error (os error 5). Checked 14:10, next check 14:20</span>"), "{html}");
        assert!(html.contains("data-storage-check") && html.contains("Check again</button>"), "{html}");
        let member = json!({ "ok": false, "since": 1792426891.6, "text": "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed." });
        let html = storage_banner_html(&member, false);
        assert!(html.contains("<span>Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed.</span>"), "{html}");
        assert!(!html.contains("Check again"), "members can't check: {html}");
        // no problem, an older server (404 → nothing), or no text: the banner still says something sensible
        assert_eq!(storage_banner_html(&json!({ "ok": true, "since": null, "text": null }), true), "");
        assert_eq!(storage_banner_html(&Value::Null, false), "");
        let bare = json!({ "ok": false, "path": "/music", "op": "mkdir", "error": "Read-only file system (os error 30)" });
        assert!(storage_banner_html(&bare, true).contains("Imports are paused: /music is not writable – mkdir: Read-only file system (os error 30)."));
        assert!(storage_banner_html(&bare, false).contains("Your downloads are kept and will be added when it is fixed."));
    }
}
