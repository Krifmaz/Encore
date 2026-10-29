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

/// Whether a library album row (`tracks`: its song count) is a single: one song, which the pages show and play as a
/// song instead of opening an album page for it. The album page itself stays (Go to album, Tag this album …).
pub fn is_single(a: &Value) -> bool {
    a.i("tracks") == 1
}

/// The library album a catalogue release tile plays as a song (its `library_album_id` when the library's copy has
/// exactly one song, `library_tracks`), or None for a release with a page of its own.
pub fn single_of(a: &Value) -> Option<String> {
    (a.truthy("library_album_id") && a.i("library_tracks") == 1).then(|| a.s("library_album_id"))
}

/// The ` data-single="<album id>"` of a tile that plays its one song on a tap ([`bind_tiles`]), or "".
fn single_attr(id: Option<String>) -> String {
    id.map(|id| format!(r#" data-single="{}""#, esc(&id))).unwrap_or_default()
}

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
    // an owned single (one song in the library) plays on a tap instead of opening the release's page
    let single = single_attr(single_of(a));
    format!(r#"<div class="tile" data-href="{href}"{single}>
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

/// `libraryTile(a)`. A single ([`is_single`]) is shown as a song ("Song · artist · year") and a tap plays it in the
/// full player instead of opening its album page.
pub fn library_tile(a: &Value) -> String {
    let id = a.s("id");
    let single = is_single(a);
    format!(r#"<div class="tile" data-href="/library/album/{id}"{single_attr}>
    <div class="art">{art}
      {untagged}
      <div class="overlay"><button class="fab" data-play-album="{id}" aria-label="Play">{play}</button></div>
    </div>
    <div class="t ellipsis" title="{album}">{album_or}</div>{under}
    <div class="s ellipsis">{song}{artist}{year}</div>
  </div>"#,
        id = esc(&id),
        single_attr = single_attr(single.then(|| id.clone())),
        song = if single { "Song · " } else { "" },
        art = ui::art(&format!("/api/library/albums/{}/cover{}", a.s("id"), ui::cover_q(a)), &a.s("album"), ""),
        // a Surround album (0.35) names its format in the corner: Dolby Atmos, 5.1, 7.1
        untagged = if a.s("class") == "surround" { format!(r#"<div class="corner"><span class="badge accent">{}</span></div>"#, esc(&if a.truthy("atmos") { "Dolby Atmos".to_string() } else { a.s("quality") })) }
            else if a.truthy("untagged") { r#"<div class="corner"><span class="badge warn" title="Some tracks lack MusicBrainz tags">Untagged</span></div>"#.to_string() } else { String::new() },
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
        if let Some(tile) = dom::closest(e, "[data-href]") {
            // a single: its song plays in the full player, there is no album page to open for it
            if let Some(id) = dom::data_opt(&tile, "single").filter(|id| !id.is_empty()) { pages::library::play_single(&id); return; }
            app::navigate(&dom::data(&tile, "href"));
        }
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

/// An import that hasn't moved for 15 minutes (or whose task is gone: `import_stalled` of the list): the row offers
/// Import again, which stops it and imports the kept files at once.
pub fn stalled_import(r: &Value) -> bool {
    r.s("status") == "importing" && r.g("import_stalled") == &Value::Bool(true)
}

/// The tooltip of Import again on a stalled import (the server's words, the same on the phone and the desktop).
pub const STALLED_IMPORT: &str = "The import hasn't moved for 15 minutes";

/// Whose requests the Requests page's buttons take: the admin's User list (`user`: one person's; none: everyone's),
/// or `mine` (members, and the Downloads page). The server keeps a member to their own whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Whose {
    pub mine: bool,
    pub user: Option<i64>,
}

impl Whose {
    /// Only the caller's own.
    pub fn mine() -> Whose { Whose { mine: true, user: None } }

    /// `body` with `mine`, and `user` when one person is picked.
    pub fn body(self, mut body: Value) -> Value {
        body["mine"] = json!(self.mine);
        if let Some(u) = self.user { body["user"] = json!(u); }
        body
    }

    /// The list's query for it: "&mine=true", "&user=12", or nothing (everyone's).
    pub fn query(self) -> String {
        if self.mine { return "&mine=true".into(); }
        self.user.map(|u| format!("&user={u}")).unwrap_or_default()
    }
}

/// What Cancel and Remove from list say first when a request keeps downloaded files: they go with it (Encore's own
/// staging folder is deleted) and Import again is gone.
pub const KEPT_FILES_LOST: &str = "Its downloaded files have not been imported yet and can't be imported after this.";

/// Keep anyway / Download anyway (R2, the owner on Oct 19: "i cant overide to have it download it anyways"): what a
/// request offers for a download the audio check turned away, from the list's `anyway`.
#[derive(Debug, PartialEq)]
pub struct Anyway {
    /// the route after `/requests/{id}/`: `keep-anyway` (the kept files are imported without the check) or
    /// `download-anyway` (the files are gone: the same source and link are asked for again without the check)
    pub act: &'static str,
    pub label: &'static str,
    pub title: &'static str,
    /// the toast once the server took it
    pub done: &'static str,
    /// which rejected download (`{rejected: id}`), so the server acts on the one the page showed
    pub id: Value,
    /// why the check turned it away, in full: "The audio sounds like Diase – Sun, not Moonlight Shadow"
    pub text: String,
}

/// The offer of a request row (`anyway` of `GET /api/requests`), or None. The server offers it only where it can
/// be taken (a failed request, or one still looking); the page shows the button to those who may take it
/// ([`may_fix`]).
pub fn anyway(r: &Value) -> Option<Anyway> {
    let a = r.g("anyway");
    let (act, label, title, done) = match a.s("action").as_str() {
        "keep" => ("keep-anyway", "Keep anyway", "Import the download the audio check turned away, without the check", "It will be imported without the audio check"),
        "download" => ("download-anyway", "Download anyway", "Download it again from the same place, without the audio check", "Downloading it again without the audio check"),
        _ => return None,
    };
    // the server's sentence; else the rejected download's own
    let text = if a.truthy("text") { a.s("text") } else {
        r.a("rejected").iter().find(|e| e.g("id") == a.g("id")).map(|e| if e.truthy("text") { e.s("text") } else { e.s("reason") }).unwrap_or_default()
    };
    Some(Anyway { act, label, title, done, id: a.g("id").clone(), text })
}

/// Keep anyway and Download anyway are taken as Retry is: by admins, and by the request's owner with the `fix`
/// permission (members see only their own requests). Others would only get "Ask an admin to keep it anyway".
/// The same people get Retry, Retry failed and the picker's "Don't check the audio".
pub fn may_fix() -> bool {
    app::is_admin() || app::user().is_some_and(|u| u.g("perms").truthy("fix"))
}

/// The Keep anyway / Download anyway button of a row or of its details.
fn anyway_button(a: &Anyway, class: &str) -> String {
    format!(r#"<button class="{class}" data-act="{}" title="{}">{}{}</button>"#, a.act, esc(a.title), icon(if a.act == "keep-anyway" { "check" } else { "download" }), a.label)
}

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
    // a download the audio check turned away: the reason in full under the line, and Keep anyway / Download anyway
    let offer = anyway(r);
    // (a hand pick the check turned away has the reason as its message: it shows once, in full)
    let same = offer.as_ref().is_some_and(|a| a.text.trim() == r.s("message").trim());
    let shown_message = r.truthy("message") && !same;
    format!(r#"<div class="list-item req-row" data-id="{id}">
    <div class="thumb"{play}>{thumb}</div>
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
      {why}
      {progress}
    </div>
    <div class="row">
      {badge}
      {approve}
      {pick}
      {keep}
      {retry}
      {now}
      {cancel}
      {menu}
    </div>
  </div>"#,
        id = r.s("id"),
        thumb = ui::art(&r.s("cover_url"), &or(r, "title", "artist"), ""),
        // a finished request's cover plays it (the details' Play); the rest of the row still opens the details
        play = if can_play(r) { r#" data-act="play" title="Play" role="button""# } else { "" },
        kind = esc(&kind),
        // the dot after attempts and source only when something follows it (a hand pick's reason shows on its own line)
        attempts = { let n = r.f("attempts") as i64; if n > 1 { format!(r#"<span class="faint" title="Requested or retried {n} times">{n} attempts</span>{}"#, if r.truthy("source") || shown_message { " ·" } else { "" }) } else { String::new() } },
        source = if r.truthy("source") { format!(r#"<span class="faint">via {}</span>{}"#, esck(r, "source"), if shown_message { " ·" } else { "" }) } else { String::new() },
        message = if shown_message { esck(r, "message") } else { String::new() },
        full = if r.truthy("message") { format!(r#" title="{}""#, esck(r, "message")) } else { String::new() },
        why = match &offer { Some(a) if !a.text.trim().is_empty() => format!(r#"<div class="small req-why">{}{}</div>"#, icon("alert"), esc(&a.text)), _ => String::new() },
        keep = match &offer { Some(a) if may_fix() => anyway_button(a, "btn sm"), _ => String::new() },
        by = if admin && r.truthy("username") { format!(r#" · <span class="faint">by {}</span>"#, esck(r, "username")) } else { String::new() },
        ago = ui::fmt::ago(r.f("created_at")),
        progress = if active && r.truthy("progress") { format!(r#"<div class="progress" style="margin-top:6px;max-width:420px"><i style="width:{}%"></i></div>"#, r.s("progress")) } else { String::new() },
        badge = request_badge(r),
        approve = if admin && status == "pending" { format!(r#"<button class="btn sm primary" data-act="approve">{}Approve</button><button class="btn sm danger" data-act="deny">Deny</button>"#, icon("check")) } else { String::new() },
        pick = if matches!(kind.as_str(), "album" | "track") && status != "completed" { format!(r#"<button class="btn sm" data-act="pick" data-req='{}' title="Search the sources and pick the exact download">{}Pick</button>"#, esc(&pick_json(r)), icon("search")) } else { String::new() },
        // a failed or waiting import whose files are kept: Import again takes Retry's place; an import that hasn't moved
        // for 15 minutes is stopped and imported again
        retry = if can_import_again(r) { format!(r#"<button class="btn sm" data-act="import-again" title="Import the downloaded files again – nothing is downloaded again">{}Import again</button>"#, icon("refresh")) }
            else if stalled_import(r) { format!(r#"<button class="btn sm" data-act="import-again" title="{STALLED_IMPORT}">{}Import again</button>"#, icon("refresh")) }
            // a playlist's songs anyone may queue again; any other request needs Fix their own downloads (the
            // server answers "Ask an admin to retry")
            else if kind == "playlist" || (matches!(status.as_str(), "failed" | "cancelled") && may_fix()) { format!(r#"<button class="btn sm" data-act="retry" title="Retry">{}</button>"#, icon("refresh")) } else { String::new() },
        now = download_now_button(r),
        cancel = if matches!(status.as_str(), "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing" | "waiting") { format!(r#"<button class="btn sm" data-act="cancel" title="Cancel">{}</button>"#, icon("x")) } else { String::new() },
        menu = if !active { format!(r#"<button class="btn sm ghost icon" data-act="menu" data-status="{}"{} aria-label="More">{}</button>"#, esc(&status), if r.truthy("has_files") { r#" data-files="1""# } else { "" }, icon("more")) } else { String::new() })
}

/// A request as a tile for Home's sideways row (like Recently added): the cover with the status in the corner and
/// Download now / Keep anyway / Cancel over it on hover, what and who, the progress. A click opens the details.
pub fn request_tile(r: &Value) -> String {
    ROWS.with(|m| m.borrow_mut().insert(r.s("id"), r.clone()));
    let kind = r.s("kind");
    let status = r.s("status");
    let (what, by) = match kind.as_str() {
        "album" => (or(r, "title", "album"), r.s("artist")),
        "artist" => (r.s("artist"), "full discography".to_string()),
        "url" => (or(r, "title", "url"), String::new()),
        "playlist" => (r.s("title"), "playlist import".to_string()),
        _ => (r.s("title"), r.s("artist")),
    };
    let active = matches!(status.as_str(), "searching" | "downloading" | "importing" | "monitoring");
    let keep = match anyway(r) { Some(a) if may_fix() => anyway_button(&a, "btn sm"), _ => String::new() };
    let cancel = if matches!(status.as_str(), "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing" | "waiting") { format!(r#"<button class="btn sm" data-act="cancel" title="Cancel">{}</button>"#, icon("x")) } else { String::new() };
    format!(r#"<div class="tile req-tile" data-id="{id}">
  <div class="art" data-act="details">{art}
    <div class="corner">{badge}</div>
    <div class="overlay">{now}{keep}{cancel}</div>
  </div>
  <div class="t ellipsis" data-act="details" title="{what}">{what}</div>
  <div class="s ellipsis">{sub}</div>
  {progress}
</div>"#,
        id = r.s("id"),
        art = ui::art(&r.s("cover_url"), &or(r, "title", "artist"), ""),
        badge = request_badge(r),
        now = download_now_button(r),
        what = esc(&what),
        sub = esc(&[by, kind].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")),
        progress = if active && r.truthy("progress") { format!(r#"<div class="progress" style="margin-top:6px"><i style="width:{}%"></i></div>"#, r.s("progress")) } else { String::new() })
}

/// Download now (the owner on Oct 21: "for the que can i get a download now button"): the row's button, where the
/// server offers it (`can_download_now`, with what it does as the tooltip: "Go first in line for Deezer"). An older
/// server has no such field: no button.
pub fn download_now_button(r: &Value) -> String {
    if r.g("can_download_now") != &Value::Bool(true) { return String::new(); }
    let title = r.g("download_now").s("title");
    format!(r#"<button class="btn sm" data-act="download-now" title="{}">{}Download now</button>"#, esc(if title.is_empty() { "Download now" } else { &title }), icon("download"))
}

/// The toast kind of Download now's answer: `ok` when something changed or started, `warn` when nothing could be
/// sooner (the answer says when).
pub fn download_now_kind(v: &Value) -> &'static str {
    if v.g("ok") == &Value::Bool(true) { "ok" } else { "warn" }
}

/// What the toast says when the server answered a press without words.
pub const STARTING_NOW: &str = "Starting now";
/// Download all now with nothing to do.
pub const NOTHING_WAITING: &str = "Nothing is waiting – every request is downloading or done";

/// The toast after Download now: the server's words and their kind ([`download_now_kind`]); an answer without words
/// says "Starting now" when it started something, and nothing when it changed nothing (as the phone and the desktop).
pub fn download_now_toast(v: &Value) -> Option<(String, &'static str)> {
    let text = v.s("text").trim().to_string();
    if text.is_empty() {
        return (download_now_kind(v) == "ok").then(|| (STARTING_NOW.to_string(), "ok"));
    }
    Some((text, download_now_kind(v)))
}

/// The toast after Download all now: the server's words; without them "Starting now", or "Nothing is waiting – every
/// request is downloading or done" when nothing was waiting.
pub fn download_all_now_toast(v: &Value) -> (String, &'static str) {
    let text = v.s("text").trim().to_string();
    let kind = download_now_kind(v);
    if !text.is_empty() {
        return (text, kind);
    }
    ((if kind == "ok" { STARTING_NOW } else { NOTHING_WAITING }).to_string(), kind)
}

/// Press Download now on request `id`: the server's words as a toast, then the lists reload; an error's words, and the
/// button back.
fn press_download_now(id: &str, btn: Element, reload: Rc<dyn Fn()>) {
    dom::set_disabled(&btn, true);
    let path = format!("/requests/{id}/download-now");
    dom::spawn(async move {
        match api::post(&path, json!({})).await {
            Ok(v) => {
                if let Some((text, kind)) = download_now_toast(&v) {
                    toast(&text, kind);
                }
                dom::dispatch("encore:requests-changed", None);
                reload();
            }
            Err(err) => {
                toast(&err.message, "err");
                dom::set_disabled(&btn, false);
            }
        }
    });
}

/// Download all now (admins, the Requests page): every waiting request is woken and the lines move up – never more
/// at once than each source allows (`whose`: only the admin's own, or one person's, requests move up).
pub fn download_all_now(whose: Whose, reload: Rc<dyn Fn()>) {
    dom::spawn(async move {
        match api::post("/requests/download-now", whose.body(json!({}))).await {
            Ok(v) => {
                let (text, kind) = download_all_now_toast(&v);
                toast(&text, kind);
                dom::dispatch("encore:requests-changed", None);
                reload();
            }
            Err(e) => toast(&e.message, "err"),
        }
    });
}

/// The tooltip of Download all now.
pub const DOWNLOAD_ALL_NOW_TITLE: &str = "Wake every waiting request and move requests up in their lines – never more at once than each source allows";

/// `bindRequestActions(root, reload)`: approve / deny / retry / cancel / pick / Download now, the ⋯ menu of request
/// rows, and the cover of a finished request, which plays it.
pub fn bind_request_actions(root: &Element, reload: Rc<dyn Fn()>) {
    dom::on(root, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-act]") else { return };
        let Some(row) = dom::closest_of(&b, "[data-id]") else { return };
        let id = dom::data(&row, "id");
        let act = dom::data(&b, "act");
        if act == "details" {
            // a click that ends a text selection on the row is not a request for the dialog
            if dom::closest(e, "a, button").is_some() || details::selecting() { return; }
            show_request_details(&id, reload.clone());
            return;
        }
        if act == "play" {
            // the cover of a finished request: play what it brought into the library
            if let Some(r) = drawn_row(&id) { play_request(&r); }
            return;
        }
        if act == "keep-anyway" || act == "download-anyway" {
            take_anyway(&id, &act, b, reload.clone(), None);
            return;
        }
        if act == "download-now" {
            press_download_now(&id, b, reload.clone());
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
            let (id2, reload2) = (id.clone(), reload.clone());
            let mut items = vec![
                MenuItem::new("Details", &icon("list"), move || show_request_details(&id2, reload2.clone())),
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

/// Keep anyway / Download anyway (`act` is the route) for request `id`, on the rejected download the page showed:
/// the server's words as a toast, then the lists reload (and `modal`, the details it was pressed in, closes).
fn take_anyway(id: &str, act: &str, btn: Element, reload: Rc<dyn Fn()>, modal: Option<Rc<ui::Modal>>) {
    let Some(a) = drawn_row(id).as_ref().and_then(anyway).filter(|a| a.act == act) else { return };
    dom::set_disabled(&btn, true);
    let path = format!("/requests/{id}/{act}");
    dom::spawn(async move {
        let body = if a.id.is_null() { json!({}) } else { json!({ "rejected": a.id }) };
        match api::post(&path, body).await {
            Ok(_) => {
                toast(a.done, "ok");
                if let Some(m) = &modal { m.close(); }
                dom::dispatch("encore:requests-changed", None);
                reload();
            }
            Err(err) => {
                // "The rejected download is gone – Download anyway asks for it again": the row is redrawn with what
                // the server offers now
                toast(&err.message, "err");
                dom::set_disabled(&btn, false);
                reload();
            }
        }
    });
}

/// Details of a request row: everything the row cuts off, in a dialog with Copy. A download the audio check turned
/// away shows its reason in full at the top, with Keep anyway / Download anyway beside Copy.
fn show_request_details(id: &str, reload: Rc<dyn Fn()>) {
    let Some(r) = drawn_row(id) else { return };
    let text = request_text(&r, &app::info().s("version"), &ui::fmt::date);
    let what = request_what(&r);
    let title = if what.is_empty() { "Details" } else { &what };
    let offer = anyway(&r);
    let take = match &offer { Some(a) if may_fix() => anyway_button(a, "btn primary"), _ => String::new() };
    let play = if can_play(&r) { format!(r#"<button class="btn primary" data-act="play">{}Play</button>"#, icon("play")) } else { String::new() };
    let body = format!("{}{}{}", request_links_html(&r), offer.as_ref().map(|a| anyway_box_html(&r, a)).unwrap_or_default(), details::body_html(&text));
    let footer = format!(r#"<button class="btn" data-close>Close</button><button class="btn{}" data-copy-text="{}">{}Copy</button>{play}{take}"#,
        if play.is_empty() && take.is_empty() { " primary" } else { "" }, esc(&text), icon("copy"));
    let m = Rc::new(ui::modal(ui::ModalOpts::new(&esc(title), &body).footer(&footer).wide(true)));
    let (el, id, m2) = (m.el.clone(), id.to_string(), m.clone());
    dom::on(&el, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-act]") else { return };
        let act = dom::data(&b, "act");
        if act == "play" {
            m2.close();
            if let Some(r) = drawn_row(&id) { play_request(&r); }
            return;
        }
        take_anyway(&id, &act, b, reload.clone(), Some(m2.clone()));
    });
}

/// Where a request's title goes in its details (the owner on Oct 23: "from the download i should be able to click on
/// the artist song or album or play it if its downloaded"): the library album once it is there (a song's album), else
/// the catalogue's album page for an album asked for by its release group, else a search for it.
#[derive(Debug, PartialEq)]
pub enum RequestTarget {
    LibraryAlbum(String),
    CatalogAlbum(String),
    /// (what to search for, the search's type: `album` or `track`)
    Search(String, String),
}

/// The request's title as a link; None for an artist (its name is the link), a link or a playlist.
pub fn title_target(r: &Value) -> Option<RequestTarget> {
    let kind = r.s("kind");
    if kind != "album" && kind != "track" { return None; }
    if r.truthy("library_album_id") { return Some(RequestTarget::LibraryAlbum(r.s("library_album_id"))); }
    if kind == "album" && r.truthy("mbid") { return Some(RequestTarget::CatalogAlbum(r.s("mbid"))); }
    let title = if kind == "album" { or(r, "title", "album") } else { r.s("title") };
    if title.is_empty() { return None; }
    let q = [r.s("artist"), title].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
    Some(RequestTarget::Search(q, kind))
}

/// The artist a request's details link to ("" for a link or a playlist, which name none).
pub fn artist_target(r: &Value) -> String {
    if matches!(r.s("kind").as_str(), "url" | "playlist") { String::new() } else { r.s("artist") }
}

/// Play in a request's details: a finished album or song request whose music is in the library.
pub fn can_play(r: &Value) -> bool {
    r.s("status") == "completed" && (r.i("library_track_id") > 0 || (r.s("kind") == "album" && r.truthy("library_album_id")))
}

/// The top of a request's details: what was asked for as links – the album (or song's album) and the artist.
fn request_links_html(r: &Value) -> String {
    let enc = |s: &str| String::from(js_sys::encode_uri_component(s));
    let artist = artist_target(r);
    let artist_link = if artist.is_empty() { String::new() } else {
        format!(r##"<a class="artist-link" data-close href="#/artist-name/{}">{}</a>"##, enc(&artist), esc(&artist))
    };
    let title = match title_target(r) {
        Some(t) => {
            let href = match &t {
                RequestTarget::LibraryAlbum(id) => format!("#/library/album/{}", enc(id)),
                RequestTarget::CatalogAlbum(rg) => format!("#/album/{}", enc(rg)),
                RequestTarget::Search(q, kind) => format!("#/search?q={}&type={kind}", enc(q)),
            };
            let what = if r.s("kind") == "album" { or(r, "title", "album") } else { r.s("title") };
            format!(r##"<a data-close href="{href}" style="font-weight:600">{}</a>"##, esc(&what))
        }
        None => String::new(),
    };
    let parts: Vec<String> = [title, artist_link].into_iter().filter(|x| !x.is_empty()).collect();
    if parts.is_empty() { return String::new(); }
    format!(r#"<div class="req-links" style="margin-bottom:10px">{}</div>"#, parts.join(r#" <span class="muted">·</span> "#))
}

/// Plays what a finished request brought into the library: the song, or the album from its first song.
fn play_request(r: &Value) {
    if r.i("library_track_id") > 0 {
        let id = r.i("library_track_id");
        dom::spawn(async move {
            match api::get(&format!("/library/tracks/{id}")).await {
                Ok(t) => player::play_queue(vec![t], 0, "list"),
                Err(e) => toast(&e.message, "err"),
            }
        });
    } else if r.truthy("library_album_id") {
        pages::library::play_album(&r.s("library_album_id"), 0);
    }
}

/// The top of a request's details when the audio check turned a download away: why, in full, and where it came from.
pub fn anyway_box_html(r: &Value, a: &Anyway) -> String {
    let e = r.a("rejected").into_iter().find(|e| e.g("id") == &a.id).unwrap_or(Value::Null);
    let from = rejected_from(&e);
    format!(r#"<div class="anyway-box">{}<div><div class="anyway-text">{}</div>{}</div></div>"#,
        icon("alert"), esc(&a.text), if from.is_empty() { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(&from)) })
}

/// Where a rejected download came from, in one line: "Deezer · via streamrip · https://www.deezer.com/track/1 ·
/// 01. Moonlight Shadow.flac".
fn rejected_from(e: &Value) -> String {
    let files = match e.i("count") {
        n if n > 1 && e.truthy("file") => format!("{} and {} more", e.s("file"), n - 1),
        _ => e.s("file"),
    };
    [e.s("via"), e.s("detail"), e.s("url"), files].into_iter().filter(|x| !x.trim().is_empty()).collect::<Vec<_>>().join(" · ")
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
        "waiting_disk" => "the downloads folder can't be written now",
        "plugin_install" => "its source's plugin is still being installed",
        _ => "",
    }
}

/// The status of a request waiting for a downloads folder (`wait_reason` "waiting_disk", backlog item 17).
pub const WAITING_DISK: &str = "Waiting for the downloads folder";

/// A request row's badge: its status, and for a request waiting for a downloads folder that it is.
pub fn request_badge(r: &Value) -> String {
    if r.s("status") == "waiting" && r.s("wait_reason") == "waiting_disk" {
        return format!(r#"<span class="badge warn">{}</span>"#, esc(WAITING_DISK));
    }
    status_badge(&r.s("status"))
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
    // Download now: where it stands in its source's line, its place in a Soulseek user's upload queue, a torrent
    // without progress, and who asked for it to go first
    let line = r.g("line");
    if line.is_object() && line.truthy("text") {
        add("In line", if line.truthy("limit_text") { format!("{} ({})", line.s("text"), line.s("limit_text")) } else { line.s("text") });
    }
    let rq = r.g("remote_queue");
    if rq.is_object() && rq.truthy("text") {
        add("Soulseek queue", if rq.f("since") > 0.0 { format!("{} (since {})", rq.s("text"), date(rq.f("since"))) } else { rq.s("text") });
    }
    if r.f("stalled_since") > 0.0 { add("No progress since", date(r.f("stalled_since"))); }
    let pressed = r.g("pressed");
    if pressed.is_object() && pressed.truthy("by") {
        add("Download now", if pressed.f("at") > 0.0 { format!("asked by {} at {}", pressed.s("by"), date(pressed.f("at"))) } else { format!("asked by {}", pressed.s("by")) });
    }
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
    if r.truthy("skip_check") { add("Audio check", "off for this request".into()); }
    // the downloads the audio check turned away (R2), oldest first: why in full, where from, and whether the files are kept
    let rejected = r.a("rejected");
    if !rejected.is_empty() {
        out.push(String::new());
        out.push("Turned away by the audio check (oldest first):".into());
    }
    for e in rejected {
        let why = if e.truthy("text") { e.s("text") } else { e.s("reason") };
        out.push(if why.trim().is_empty() { "The audio sounds like another song".to_string() } else { why });
        let mut sub = |k: &str, v: String| { if !v.trim().is_empty() { out.push(format!("  {k}: {v}")); } };
        sub("From", rejected_from(&e));
        if e.truthy("text") && e.truthy("reason") { sub("Check", e.s("reason")); }
        if e.f("at") > 0.0 { sub("When", date(e.f("at"))); }
        sub("Files", if !e.truthy("has_file") { "gone".into() } else if e.truthy("path") { format!("kept in {}", e.s("path")) } else { "kept on the server".into() });
    }
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
/// to 30 s after the request shows Available); Check again probes at once – the download folders too, so the
/// banner on every page ([`crate::notice`]) takes its answer, and this one is drawn again after that one's.
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
    dom::on(&dom::window(), "encore:storage-checked", { let load = load.clone(); move |_| load() });
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
                    crate::notice::keep(&s);
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
/// optionally deleting their files (`whose`: the caller's own, one person's or everyone's).
pub fn clear_menu(anchor: &Element, reload: Rc<dyn Fn()>, whose: Whose) {
    let admin = app::is_admin();
    let go = Rc::new(move |statuses: &'static [&'static str], files: bool, label: &'static str| {
        let reload = reload.clone();
        dom::spawn(async move {
            if files && !ui::confirm_dialog("Delete files", &format!("{label}: this permanently deletes the files from disk."), "Delete", true).await { return; }
            match api::post("/requests/clear", whose.body(json!({ "statuses": statuses, "files": files }))).await {
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

/// The question before Clear waiting: "Cancel 612 waiting requests? Downloads and imports already running keep going."
pub fn waiting_question(n: i64) -> String {
    if n == 1 { "Cancel 1 waiting request? Downloads and imports already running keep going.".to_string() }
    else { format!("Cancel {n} waiting requests? Downloads and imports already running keep going.") }
}

/// What Clear waiting did, when the server's own words are missing: "Cancelled 612 waiting requests", "Nothing is
/// waiting" (the server's sentences, the same).
pub fn waiting_done(n: i64) -> String {
    match n {
        0 => "Nothing is waiting".to_string(),
        1 => "Cancelled 1 waiting request".to_string(),
        n => format!("Cancelled {n} waiting requests"),
    }
}

/// Clear waiting: every request still waiting (pending, queued, waiting, searching) is cancelled – admins: everyone's,
/// one person's (`whose.user`) or their own (`whose.mine`); members: their own (the server decides). Counted first (a
/// dry run) for the question; the server's words after ("Cancelled 612 waiting requests", "Nothing is waiting").
pub fn clear_waiting(whose: Whose, reload: Rc<dyn Fn()>) {
    dom::spawn(async move {
        let n = match api::post("/requests/clear-waiting", whose.body(json!({ "dry_run": true }))).await {
            Ok(r) if r.i("count") > 0 => r.i("count"),
            Ok(r) => { let text = r.s("text"); toast(if text.is_empty() { waiting_done(0) } else { text }.as_str(), ""); return; }
            Err(e) => { toast(&e.message, "err"); return; }
        };
        if !ui::confirm_dialog("Clear waiting", &waiting_question(n), "Clear waiting", true).await { return; }
        match api::post("/requests/clear-waiting", whose.body(json!({}))).await {
            Ok(r) => {
                let text = r.s("text");
                toast(if text.is_empty() { waiting_done(r.i("count")) } else { text }.as_str(), "ok");
                dom::dispatch("encore:requests-changed", None);
                reload();
            }
            Err(e) => toast(&e.message, "err"),
        }
    });
}

/// Retry failed: queue every failed request again (admins: everyone's, one person's or their own: `whose`), say how
/// many, reload.
pub fn retry_failed(whose: Whose, reload: Rc<dyn Fn()>) {
    dom::spawn(async move {
        match api::post("/requests/retry-failed", whose.body(json!({}))).await {
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

    /// A one-song album is a single: its tile says "Song", carries the album to play and opens no album page; an
    /// album of two songs or more, and a row without a count, keep their page.
    #[test]
    fn a_single_plays_instead_of_opening_an_album_page() {
        let one = json!({ "id": "a1", "album": ">one (greater than one)", "albumartist": "VALORANT & ericdoa", "year": "2023", "tracks": 1 });
        let html = library_tile(&one);
        assert!(html.contains(r#"data-single="a1""#) && html.contains("Song · VALORANT"), "{html}");
        for a in [json!({ "id": "a2", "album": "Two", "tracks": 2 }), json!({ "id": "a3", "album": "No count" })] {
            let html = library_tile(&a);
            assert!(!html.contains("data-single") && !html.contains("Song · "), "{html}");
        }
        // a release of the artist's discography: only when the library's copy has one song
        let rel = |n: i64| json!({ "id": "rg", "title": ">one", "library_album_id": "a1", "library_tracks": n });
        assert_eq!(single_of(&rel(1)).as_deref(), Some("a1"));
        assert_eq!(single_of(&rel(3)), None);
        assert_eq!(single_of(&json!({ "id": "rg", "library_tracks": 1 })), None, "not in the library");
        assert!(album_tile_with(&rel(1), "/album/rg", "", None, None, None).contains(r#"data-single="a1""#));
        assert!(!album_tile_with(&rel(2), "/album/rg", "", None, None, None).contains("data-single"));
    }

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
        // a plain failure keeps Retry, for those who may retry (Fix their own downloads, as the phone has it); a
        // Soulseek wait has no files and no Import again
        f["has_files"] = json!(false);
        crate::app::set_user(Some(json!({ "role": "user", "username": "mia", "perms": { "fix": true } })));
        let html = request_row(&f, false);
        assert!(html.contains(r#"data-act="retry""#) && !html.contains("Import again"), "{html}");
        crate::app::set_user(Some(json!({ "role": "user", "username": "mia", "perms": { "fix": false } })));
        assert!(!request_row(&f, false).contains(r#"data-act="retry""#), "the server would answer \"Ask an admin to retry\"");
        let mut pl = f.clone();
        pl["kind"] = json!("playlist");
        assert!(request_row(&pl, false).contains(r#"data-act="retry""#), "a playlist's songs anyone may queue again");
        crate::app::set_user(None);
        let s = json!({ "id": 13, "kind": "track", "status": "waiting", "title": "Blow", "artist": "Ke$ha", "created_at": 0, "attempts": 1,
            "message": "Waiting for Soulseek – next attempt at 14:20", "wait_reason": "soulseek", "has_files": false });
        let html = request_row(&s, false);
        assert!(html.contains(r#"<span class="badge warn">Waiting</span>"#) && !html.contains("Import again") && html.contains(r#"data-act="cancel""#), "{html}");
        assert_eq!(wait_words("soulseek"), "Soulseek isn't connected yet");
        assert_eq!(wait_words("plugin_install"), "its source's plugin is still being installed");
        // a link's request that kept the files it could not import offers Import again for them
        let c = json!({ "id": 14, "kind": "url", "status": "completed", "title": "Mix", "created_at": 0, "has_files": true,
            "message": "Downloaded 1 of 2 file(s) – the rest could not be imported: …" });
        assert!(can_import_again(&c) && request_row(&c, true).contains("Import again</button>"));
        assert!(!can_import_again(&json!({ "status": "completed", "has_files": false })));
    }

    /// Home's downloads are tiles: the status in the corner, Cancel over the cover, the progress under the title.
    #[test]
    fn a_request_tile_has_its_status_cancel_and_progress() {
        let r = json!({ "id": 21, "kind": "track", "status": "downloading", "title": "In Time", "artist": "JustWarrenPeace", "created_at": 0,
            "attempts": 1, "progress": 40, "has_files": false });
        let html = request_tile(&r);
        assert!(html.contains(r#"class="tile req-tile" data-id="21""#) && html.contains(r#"data-act="details""#), "{html}");
        assert!(html.contains(r#"data-act="cancel""#) && html.contains(r#"style="width:40%""#), "{html}");
        assert!(html.contains("JustWarrenPeace · track"), "{html}");
        let done = json!({ "id": 22, "kind": "album", "status": "completed", "title": "", "album": "Hold", "artist": "HOME", "created_at": 0 });
        let html = request_tile(&done);
        assert!(html.contains(">Hold<") && !html.contains(r#"data-act="cancel""#) && !html.contains("progress"), "{html}");
    }

    /// An import that hasn't moved for 15 minutes offers Import again, with why; one that moves doesn't.
    #[test]
    fn a_stalled_import_offers_import_again() {
        let mut r = json!({ "id": 15, "kind": "album", "status": "importing", "title": "Illuminate EP", "artist": "Duumu", "created_at": 0, "attempts": 1,
            "progress": 100, "message": "Importing – checking the audio (3 of 12)", "has_files": true, "import_stalled": true, "import_moved_at": 1792427000.0 });
        assert!(stalled_import(&r));
        let html = request_row(&r, true);
        assert!(html.contains(r#"data-act="import-again" title="The import hasn't moved for 15 minutes""#) && html.contains("Import again</button>"), "{html}");
        assert!(html.contains(r#"data-act="cancel""#), "{html}");
        r["import_stalled"] = json!(false);
        assert!(!stalled_import(&r));
        assert!(!request_row(&r, true).contains("Import again"));
        // an older server says nothing: no button
        assert!(!stalled_import(&json!({ "status": "importing" })));
        assert!(!stalled_import(&json!({ "status": "failed", "import_stalled": true })));
    }

    /// The bulk buttons' bodies and the list's query: one person's, everyone's or the caller's own.
    #[test]
    fn whose_requests_the_buttons_take() {
        assert_eq!(Whose::default().body(json!({ "dry_run": true })), json!({ "dry_run": true, "mine": false }));
        assert_eq!(Whose { mine: false, user: Some(12) }.body(json!({})), json!({ "mine": false, "user": 12 }));
        assert_eq!(Whose::mine().body(json!({})), json!({ "mine": true }));
        assert_eq!(Whose::default().query(), "");
        assert_eq!(Whose { mine: false, user: Some(12) }.query(), "&user=12");
        assert_eq!(Whose::mine().query(), "&mine=true");
    }

    /// The cover of a finished request plays it; a request with nothing in the library yet keeps a plain cover, and
    /// the text beside it opens the details either way.
    #[test]
    fn the_cover_of_a_finished_request_plays_it() {
        let done = json!({ "id": 31, "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "status": "completed", "library_album_id": "abc" });
        let html = request_row(&done, false);
        assert!(html.contains(r#"<div class="thumb" data-act="play" title="Play" role="button">"#), "{html}");
        assert!(html.contains(r#"class="grow req-text" data-act="details""#), "{html}");
        let queued = json!({ "id": 32, "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "status": "queued" });
        let html = request_row(&queued, false);
        assert!(html.contains(r#"<div class="thumb">"#) && !html.contains(r#"data-act="play""#), "{html}");
    }

    #[test]
    fn the_details_link_the_album_the_artist_and_play_what_is_downloaded() {
        // the Love Buzz card of the Oct 23 screenshot: queued, asked for by its release group
        let queued = json!({ "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "mbid": "rg-1", "status": "queued" });
        assert_eq!(title_target(&queued), Some(RequestTarget::CatalogAlbum("rg-1".into())));
        assert_eq!(artist_target(&queued), "Nirvana");
        assert!(!can_play(&queued));
        let done = json!({ "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "mbid": "rg-1", "status": "completed", "library_album_id": "abc", "library_track_id": null });
        assert_eq!(title_target(&done), Some(RequestTarget::LibraryAlbum("abc".into())));
        assert!(can_play(&done));
        let song = json!({ "kind": "track", "artist": "Nirvana", "title": "Blew", "status": "failed" });
        assert_eq!(title_target(&song), Some(RequestTarget::Search("Nirvana Blew".into(), "track".into())));
        let song_done = json!({ "kind": "track", "artist": "Nirvana", "title": "Blew", "status": "completed", "library_album_id": "x", "library_track_id": 12 });
        assert_eq!(title_target(&song_done), Some(RequestTarget::LibraryAlbum("x".into())));
        assert!(can_play(&song_done));
        // finished, but no longer in the library: nothing to play
        assert!(!can_play(&json!({ "kind": "track", "title": "Blew", "status": "completed", "library_track_id": null })));
        let link = json!({ "kind": "url", "url": "https://example.com/a", "artist": "x", "status": "completed" });
        assert_eq!(title_target(&link), None);
        assert_eq!(artist_target(&link), "");
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

    /// A row of GET /api/requests after the audio check turned the download away (package A's answer, R2).
    fn rejected_row() -> Value {
        json!({ "id": 21, "kind": "track", "status": "failed", "title": "Moonlight Shadow (nightcore)", "artist": "Machine Time", "album": "Moonlight Shadow (nightcore)",
            "source": "ytdlp", "username": "alice", "created_at": 0, "attempts": 10, "picked": false, "has_files": false, "skip_check": false,
            "message": "All sources exhausted (ytdlp: YouTube match was the wrong audio (audio is “Diase – Sun” (different title)))",
            "rejected": [
                { "id": 1, "source": "streamrip", "via": "Deezer", "detail": "via streamrip", "url": "https://www.deezer.com/track/3135556", "file": "01. Moonlight Shadow.flac",
                  "count": 1, "check": "audio", "reason": "audio is “Diase – Sun” (different title)", "heard": "Diase – Sun",
                  "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)", "at": 1760890000.5, "has_file": false, "path": null },
                { "id": 2, "source": "ytdlp", "via": "YouTube Music", "detail": null, "url": "https://music.youtube.com/watch?v=x", "file": "Moonlight Shadow.opus",
                  "count": 1, "check": "audio", "reason": "audio is “Diase – Sun” (different title)", "heard": "Diase – Sun",
                  "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)", "at": 1760890100.0, "has_file": true, "path": "/downloads/encore/rejected-21/2" }
            ],
            "anyway": { "action": "keep", "id": 2, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)" },
            "log": ["ytdlp: YouTube match was the wrong audio"] })
    }

    #[test]
    fn a_download_the_audio_check_turned_away_offers_keep_anyway_with_the_reason_in_full() {
        crate::app::set_user(Some(json!({ "role": "admin", "username": "caz" })));
        let r = rejected_row();
        let a = anyway(&r).expect("offered");
        assert_eq!((a.act, a.label, a.id.clone()), ("keep-anyway", "Keep anyway", json!(2)));
        assert_eq!(a.text, "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)");
        let html = request_row(&r, true);
        // the reason in full on its own line (it wraps: no ellipsis), and the button beside Retry
        assert!(html.contains(r#"<div class="small req-why">"#) && html.contains("The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)</div>"), "{html}");
        assert!(html.contains(r#"data-act="keep-anyway""#) && html.contains("Keep anyway</button>"), "{html}");
        assert!(html.contains(r#"data-act="retry""#), "Retry stays: {html}");
        // Download anyway when the kept files are gone
        let mut d = r.clone();
        d["anyway"] = json!({ "action": "download", "id": 1, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)" });
        let html = request_row(&d, true);
        assert!(html.contains(r#"data-act="download-anyway""#) && html.contains("Download anyway</button>") && !html.contains("Keep anyway"), "{html}");
        assert_eq!(anyway(&d).map(|a| a.done), Some("Downloading it again without the audio check"));
        // no offer: no line and no button
        let mut n = r.clone();
        n["anyway"] = Value::Null;
        let html = request_row(&n, true);
        assert!(!html.contains("req-why") && !html.contains("anyway"), "{html}");
        // a hand pick turned away has the reason as its message: shown once, on the line that wraps
        let mut p = r.clone();
        p["message"] = json!("The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)");
        let html = request_row(&p, true);
        assert_eq!(html.matches("The audio sounds like Diase").count(), 2, "the wrapping line and the status line's title only: {html}");
        // members: with the fix permission as Retry; without it the reason shows and the button doesn't
        crate::app::set_user(Some(json!({ "role": "user", "perms": { "fix": true } })));
        assert!(request_row(&r, false).contains(r#"data-act="keep-anyway""#));
        crate::app::set_user(Some(json!({ "role": "user", "perms": { "fix": false } })));
        let html = request_row(&r, false);
        assert!(!html.contains(r#"data-act="keep-anyway""#) && html.contains("req-why"), "{html}");
        // an older server without the field, or an action this page doesn't know
        assert!(anyway(&json!({ "status": "failed" })).is_none());
        assert!(anyway(&json!({ "anyway": { "action": "later" } })).is_none());
        // without the server's text, the rejected download's own
        let t = anyway(&json!({ "anyway": { "action": "keep", "id": 4 }, "rejected": [{ "id": 4, "reason": "audio is “X – Y”" }] })).map(|a| a.text);
        assert_eq!(t.as_deref(), Some("audio is “X – Y”"));
        crate::app::set_user(None);
    }

    #[test]
    fn the_details_name_every_download_the_audio_check_turned_away() {
        let mut r = rejected_row();
        r["skip_check"] = json!(true);
        let t = request_text(&r, "0.31.0", &date);
        let want = "
Turned away by the audio check (oldest first):
The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)
  From: Deezer · via streamrip · https://www.deezer.com/track/3135556 · 01. Moonlight Shadow.flac
  Check: audio is “Diase – Sun” (different title)
  When: <1760890000>
  Files: gone
The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)
  From: YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus
  Check: audio is “Diase – Sun” (different title)
  When: <1760890100>
  Files: kept in /downloads/encore/rejected-21/2
";
        assert!(t.contains(&format!("Audio check: off for this request\n{want}")), "{t}");
        // members get no server folder; an album names its first file and how many more
        let mut m = rejected_row();
        m["rejected"][1]["path"] = Value::Null;
        m["rejected"][1]["count"] = json!(10);
        let t = request_text(&m, "", &date);
        assert!(t.contains("  From: YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus and 9 more\n"), "{t}");
        assert!(t.contains("  Files: kept on the server\n"), "{t}");
        // the top of the dialog: the reason in full and where it came from
        let a = anyway(&r).expect("offered");
        let top = anyway_box_html(&r, &a);
        assert!(top.contains(r#"<div class="anyway-text">The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)</div>"#), "{top}");
        assert!(top.contains("YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus"), "{top}");
        // no rejected downloads: nothing more in the text
        assert!(!request_text(&waiting_row(), "", &date).contains("Turned away"));
        // the row's line: no stray dot where the hand pick's reason (shown on its own line) was
        let mut p = rejected_row();
        p["message"] = json!("The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)");
        let html = request_row(&p, true);
        assert!(html.contains("10 attempts</span> ·") && html.contains("via ytdlp</span>\n") && !html.contains("via ytdlp</span> ·"), "{html}");
    }

    #[test]
    fn a_request_waiting_for_the_downloads_folder_says_so() {
        let mut r = json!({ "id": 21, "kind": "album", "status": "waiting", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP", "created_at": 0,
            "attempts": 1, "source": "torznab", "message": "Waiting for the downloads folder – /downloads/torrents is missing", "wait_reason": "waiting_disk",
            "next_try_at": null, "has_files": false });
        assert_eq!(request_badge(&r), r#"<span class="badge warn">Waiting for the downloads folder</span>"#);
        let html = request_row(&r, true);
        assert!(html.contains(r#"<span class="badge warn">Waiting for the downloads folder</span>"#), "{html}");
        assert!(html.contains("Waiting for the downloads folder – /downloads/torrents is missing"), "{html}");
        let text = request_text(&r, "0.33.0", &|_| "date".to_string());
        assert!(text.contains("Status: Waiting – the downloads folder can't be written now"), "{text}");
        assert!(!text.contains("Next try"), "no time: it goes on when the probe passes – {text}");
        // other waits keep the plain badge
        r["wait_reason"] = json!("soulseek");
        assert_eq!(request_badge(&r), status_badge("waiting"));
    }

    /// A row of GET /api/requests waiting in the streamrip line (Download now, Oct 21).
    fn in_line_row() -> Value {
        json!({ "id": 31, "kind": "album", "status": "queued", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP", "source": "streamrip",
            "created_at": 0, "attempts": 1, "message": "2nd in line for Deezer", "has_files": false,
            "can_download_now": true, "download_now": { "act": "line", "title": "Go first in line for Deezer", "until": null },
            "line": { "source": "streamrip", "label": "Deezer", "position": 2, "waiting": 5, "running": 1, "limit": 1, "limit_text": "1 download at a time",
                      "first": false, "text": "2nd in line for Deezer" },
            "remote_queue": null, "stalled_since": null, "pressed": null })
    }

    #[test]
    fn download_now_only_where_the_server_offers_it() {
        let r = in_line_row();
        let html = request_row(&r, false);
        let button = r#"<button class="btn sm" data-act="download-now" title="Go first in line for Deezer">"#;
        assert!(html.contains(button) && html.contains("Download now</button>"), "{html}");
        // before Cancel
        assert!(html.find(button).unwrap() < html.find(r#"data-act="cancel""#).unwrap(), "{html}");
        assert!(html.contains("2nd in line for Deezer"), "the server's message: {html}");
        // the server doesn't offer it, or an older server says nothing: no button
        let mut no = r.clone();
        no["can_download_now"] = json!(false);
        no["download_now"] = Value::Null;
        assert!(!request_row(&no, false).contains("download-now"));
        let mut old = r.clone();
        old.as_object_mut().unwrap().retain(|k, _| !["can_download_now", "download_now", "line", "remote_queue", "stalled_since", "pressed"].contains(&k.as_str()));
        assert!(!request_row(&old, true).contains("download-now"));
        // the title is the server's (it says what pressing does)
        let mut sw = r.clone();
        sw["download_now"] = json!({ "act": "switch", "title": "Switch to a user with a free upload slot – it is 12th in alice's queue" });
        assert!(request_row(&sw, true).contains(r#"title="Switch to a user with a free upload slot – it is 12th in alice&#39;s queue""#));
        // the toast: ok when something changed or started, warn when nothing could be sooner
        assert_eq!(download_now_kind(&json!({ "ok": true, "action": "line" })), "ok");
        assert_eq!(download_now_kind(&json!({ "ok": false, "action": "later", "at": 1792500000 })), "warn");
        assert!(DOWNLOAD_ALL_NOW_TITLE.contains("never more at once than each source allows"));
        // the toast is the server's words; without them a press that started something says so, one that changed
        // nothing says nothing (as the phone and the desktop)
        assert_eq!(download_now_toast(&json!({ "ok": true, "action": "line", "text": "Next in line for Deezer (1 download at a time)" })),
            Some(("Next in line for Deezer (1 download at a time)".to_string(), "ok")));
        assert_eq!(download_now_toast(&json!({ "ok": false, "action": "kept", "text": "No one with a free upload slot has it – it stays 12th in alice's queue" })),
            Some(("No one with a free upload slot has it – it stays 12th in alice's queue".to_string(), "warn")));
        assert_eq!(download_now_toast(&json!({ "ok": true, "action": "now" })), Some(("Starting now".to_string(), "ok")));
        assert_eq!(download_now_toast(&json!({ "ok": false, "action": "later", "text": " " })), None);
        assert_eq!(download_all_now_toast(&json!({ "ok": true, "pressed": 2, "woken": 1, "text": "Woke 1 waiting request" })), ("Woke 1 waiting request".to_string(), "ok"));
        assert_eq!(download_all_now_toast(&json!({ "ok": false, "pressed": 0, "woken": 0 })),
            ("Nothing is waiting – every request is downloading or done".to_string(), "warn"));
        assert_eq!(download_all_now_toast(&json!({ "ok": true })), ("Starting now".to_string(), "ok"));
    }

    #[test]
    fn request_text_has_the_line_the_queue_and_the_press() {
        let mut r = in_line_row();
        r["pressed"] = json!({ "by": "carol", "at": 1792500300.0 });
        let t = request_text(&r, "0.33.0", &date);
        assert!(t.contains("\nIn line: 2nd in line for Deezer (1 download at a time)\n"), "{t}");
        assert!(t.contains("\nDownload now: asked by carol at <1792500300>\n"), "{t}");
        assert!(!t.contains("Soulseek queue") && !t.contains("No progress since"), "{t}");
        let q = json!({ "id": 32, "kind": "album", "status": "downloading", "title": "Illuminate EP", "artist": "Duumu", "created_at": 0, "attempts": 1,
            "message": "Queued by alice – 12th in their queue, 0/12 files", "remote_queue": { "user": "alice", "place": 12, "since": 1792500000.0, "text": "12th in alice's upload queue" },
            "stalled_since": null, "line": null, "pressed": null });
        let t = request_text(&q, "", &date);
        assert!(t.contains("\nSoulseek queue: 12th in alice's upload queue (since <1792500000>)\n"), "{t}");
        let s = json!({ "id": 33, "kind": "album", "status": "downloading", "title": "Rodeo", "artist": "Travis Scott", "created_at": 0, "attempts": 1,
            "message": "qBittorrent: stalled", "stalled_since": 1792499000.0 });
        assert!(request_text(&s, "", &date).contains("\nNo progress since: <1792499000>\n"));
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
