//! Port of web/js/pages/playlists.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api::{self, ApiResult};
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, J};
use crate::player;
use crate::ui::{art, confirm_dialog, empty_state, esc, icon, modal, status_badge, toast, ModalOpts};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use web_sys::Element;

/// `SRC[k]`: the display name of a playlist source.
fn src_name(k: &str) -> Option<&'static str> {
    Some(match k {
        "spotify" => "Spotify", "ytmusic" => "YouTube Music", "deezer" => "Deezer", "tidal" => "Tidal", "qobuz" => "Qobuz",
        "apple" => "Apple Music", "amazon" => "Amazon Music", "ytdlp" => "YouTube / SoundCloud", "listenbrainz" => "ListenBrainz", "web" => "Web",
        "text" => "Pasted list", "merged" => "Merged", "manual" => "Encore", "library" => "Encore",
        "jellyfin" => "Jellyfin", "plex" => "Plex", "navidrome" => "Navidrome",
        _ => return None,
    })
}
/// `TARGETS[k]`: the display name of a sync target.
fn target_name(k: &str) -> Option<&'static str> {
    Some(match k {
        "jellyfin" => "Jellyfin / Finamp", "plex" => "Plex / Plexamp", "navidrome" => "Navidrome", "ytmusic" => "YouTube Music", "listenbrainz" => "ListenBrainz",
        _ => return None,
    })
}
/// `SRC[p.source] || p.source || ''`.
fn src_label(p: &Value) -> String {
    let source = p.s("source");
    src_name(&source).map(|s| s.to_string()).unwrap_or(source)
}
/// A row "Download missing" would ask for: not in the library, and never requested or its request is gone,
/// failed, cancelled or denied. Rows whose request is waiting, running or done are on their way already.
fn requestable(it: &Value) -> bool {
    !it.truthy("track_id") && matches!(it.st("request_status"), None | Some("failed") | Some("cancelled") | Some("denied"))
}

/// The "Download missing" toast from the route's `{requested, in_progress, failed: [{title, reason}]}`:
/// "Requested 120 · 600 already in progress · 64 couldn't be requested (why)".
pub fn missing_report_text(r: &Value) -> String {
    let mut s = format!("Requested {}", r.i("requested"));
    if r.i("in_progress") > 0 {
        s += &format!(" · {} already in progress", r.i("in_progress"));
    }
    let failed = r.a("failed");
    if !failed.is_empty() {
        let mut why: Vec<String> = vec![];
        for f in &failed {
            let reason = f.s("reason");
            if !reason.is_empty() && !why.contains(&reason) {
                why.push(reason);
            }
        }
        let more = why.len().saturating_sub(3);
        why.truncate(3);
        let mut why = why.join("; ");
        if more > 0 {
            why += &format!("; {more} more");
        }
        s += &format!(" · {} couldn't be requested", failed.len());
        if !why.is_empty() {
            s += &format!(" ({why})");
        }
    }
    s
}

/// `window.dispatchEvent(new HashChangeEvent('hashchange'))`: re-render the current page.
fn reload() {
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") { let _ = dom::window().dispatch_event(&ev); }
}

fn pl_card(p: &Value) -> String {
    let pct = if p.truthy("total") { (p.f("matched") / p.f("total") * 100.0).round() as i64 } else { 0 };
    let liked = p.s("kind") == "liked";
    let cover = if liked { String::new() } else { p.s("cover_url") };
    let targets = p.a("sync_targets");
    format!(r#"<div class="tile" data-href="/playlists/{id}">
    <label class="corner" style="z-index:2" onclick="event.stopPropagation()">{check}</label>
    <div class="art">{art}</div>
    <div class="t ellipsis">{name}</div>
    <div class="s ellipsis">{total} tracks · {pct}% in library{follow}</div>
    <div class="s ellipsis faint">{src}{sync}</div>
  </div>"#,
        id = p.s("id"),
        check = if liked { String::new() } else { format!(r#"<input type="checkbox" data-merge="{}" title="Select to merge">"#, p.s("id")) },
        art = if liked { r#"<div class="ph" style="background:var(--grad);color:#fff">♥</div>"#.to_string() } else { art(&cover, &p.s("name"), "") },
        name = esc(&p.s("name")),
        total = p.s("total"),
        follow = if p.truthy("follow") { " · following" } else { "" },
        src = esc(&src_label(p)),
        sync = if targets.is_empty() { String::new() } else {
            format!(" → {}", targets.iter().map(|t| { let t = js_string(t); esc(target_name(&t).unwrap_or(&t)) }).collect::<Vec<_>>().join(", "))
        },
    )
}

pub fn import_modal(url: &str, into: Option<i64>) {
    let has_into = into.is_some_and(|i| i != 0);
    let m = modal(ModalOpts::new(
        if has_into { "Add tracks from a link" } else { "Import playlist" },
        &format!(r#"<div class="tabs" id="imp-tabs"><button data-m="url" class="active">From a link</button><button data-m="text">Paste a list</button></div>
      <div class="mt" id="imp-url"><input class="input" id="imp-link" value="{url}" placeholder="YouTube Music, Spotify, Deezer, Tidal, Apple Music, Amazon Music, Qobuz, YouTube (Music), SoundCloud, Bandcamp, ListenBrainz…">
        <p class="small faint" style="margin:6px 0 0">Connect Spotify/Tidal on the Connections page to import private playlists and Liked Songs (e.g. <span class="mono">https://music.youtube.com/playlist?list=LM</span> or <span class="mono">https://open.spotify.com/collection/tracks</span>). Amazon Music playlists and albums import from their public share links.</p></div>
      <div class="mt hidden" id="imp-text"><textarea class="input" id="imp-lines" rows="8" placeholder="One per line:  Artist - Title"></textarea></div>
      <div class="row wrap mt">
        {name_follow}
        <label class="switch"><input type="checkbox" id="imp-req"><i></i>Download missing tracks</label>
        <select class="input" id="imp-album" style="width:auto" title="When a song is part of an album">
          <option value="">Albums: admin default</option><option value="song">Just the songs</option><option value="album">Whole albums</option>
        </select>
        <button class="btn" id="imp-preview">{search}Preview</button>
      </div>
      <div id="imp-out" class="mt"></div>"#,
            url = esc(url),
            name_follow = if has_into { "" } else { r#"<input class="input" id="imp-name" placeholder="Playlist name (optional)" style="max-width:280px">
        <label class="switch"><input type="checkbox" id="imp-follow"><i></i>Keep following (pull new tracks)</label>"# },
            search = icon("search"),
        ),
    )
    .wide(true)
    .footer(&format!(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="imp-go">{}Import</button>"#, icon("download"))));
    let mode = Rc::new(RefCell::new("url".to_string()));
    if let Some(tabs) = m.qs("#imp-tabs") {
        let (m2, mode2) = (m.clone(), mode.clone());
        dom::on(&tabs, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-m]") else { return };
            *mode2.borrow_mut() = dom::data(&b, "m");
            let md = mode2.borrow().clone();
            for x in m2.qsa("#imp-tabs button") { dom::toggle_class(&x, "active", x == b); }
            if let Some(u) = m2.qs("#imp-url") { dom::toggle_class(&u, "hidden", md != "url"); }
            if let Some(t) = m2.qs("#imp-text") { dom::toggle_class(&t, "hidden", md != "text"); }
            if let Some(l) = m2.qs("#imp-follow").and_then(|f| dom::closest_of(&f, "label")) { dom::toggle_class(&l, "hidden", md != "url"); }
        });
    }
    let body: Rc<dyn Fn() -> Value> = {
        let (m2, mode2) = (m.clone(), mode.clone());
        Rc::new(move || {
            if *mode2.borrow() == "url" { json!({ "url": dom::value_of(&m2.el, "#imp-link").trim() }) } else { json!({ "text": dom::value_of(&m2.el, "#imp-lines") }) }
        })
    };
    if let Some(pv) = m.qs("#imp-preview") {
        let (m2, body2) = (m.clone(), body.clone());
        dom::on(&pv, "click", move |_| {
            let (m3, body3) = (m2.clone(), body2.clone());
            dom::spawn(async move {
                if let Some(out) = m3.qs("#imp-out") { dom::set_html(&out, r#"<div class="empty"><span class="spinner"></span></div>"#); }
                match api::post("/playlists/preview", body3()).await {
                    Ok(p) => {
                        if let Some(n) = m3.qs("#imp-name") { if dom::value(&n).is_empty() { dom::set_value(&n, &p.s("name")); } }
                        let tracks: String = p.a("tracks").iter().map(|t| format!(r#"<div class="list-item" style="padding:7px 14px">
          <div class="grow ellipsis">{artist} – {title} <span class="faint small">{album}</span></div>
          {isrc}
          {status}</div>"#,
                            artist = esc(&t.s("artist")), title = esc(&t.s("title")),
                            album = if t.truthy("album") { format!("· {}", esc(&t.s("album"))) } else { String::new() },
                            isrc = if t.truthy("isrc") { r#"<span class="badge" title="Has ISRC – exact matching">ISRC</span>"# } else { "" },
                            status = if t.truthy("track_id") { format!(r#"<span class="badge ok" title="{}">in library</span>"#, esc(&t.s("match_reason"))) } else { r#"<span class="badge">missing</span>"#.to_string() },
                        )).collect();
                        let html = format!(r#"<div class="row wrap" style="margin-bottom:10px">
          <b>{name}</b><span class="badge">{src}</span>
          <span class="badge">{count} tracks</span><span class="badge ok">{in_library} already in library</span>
          {dups}</div>
        {warning}<div class="card flat" style="max-height:300px;overflow:auto"><div class="list">{tracks}</div></div>"#,
                            name = esc(&p.s("name")), src = esc(&src_label(&p)), count = p.s("count"), in_library = p.s("in_library"),
                            warning = if p.truthy("warning") { format!(r#"<div class="error-box" style="margin-bottom:10px">{}</div>"#, esc(&p.s("warning"))) } else { String::new() },
                            dups = if p.truthy("duplicates") { format!(r#"<span class="badge warn">{} duplicate(s) will be skipped</span>"#, p.s("duplicates")) } else { String::new() },
                        );
                        if let Some(out) = m3.qs("#imp-out") { dom::set_html(&out, &html); }
                    }
                    Err(e) => { if let Some(out) = m3.qs("#imp-out") { dom::set_html(&out, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); } }
                }
            });
        });
    }
    if let Some(go) = m.qs("#imp-go") {
        let (m2, body2, go2) = (m.clone(), body.clone(), go.clone());
        dom::on(&go, "click", move |e| {
            let btn = dom::target_el(e).unwrap_or_else(|| go2.clone());
            dom::set_disabled(&btn, true);
            dom::set_html(&btn, r#"<span class="spinner"></span> Importing…"#);
            let (m3, body3) = (m2.clone(), body2.clone());
            dom::spawn(async move {
                let mut b = body3();
                if let Some(o) = b.as_object_mut() {
                    o.insert("into".into(), json!(into));
                    o.insert("name".into(), json!(m3.qs("#imp-name").map(|n| dom::value(&n).trim().to_string()).filter(|s| !s.is_empty())));
                    o.insert("follow".into(), json!(m3.qs("#imp-follow").map(|f| dom::checked(&f)).unwrap_or(false)));
                    o.insert("request_missing".into(), json!(dom::checked_of(&m3.el, "#imp-req")));
                    let album = dom::value_of(&m3.el, "#imp-album");
                    o.insert("album_mode".into(), if album.is_empty() { Value::Null } else { Value::String(album) });
                }
                match api::post("/playlists/import", b).await {
                    Ok(r) => {
                        toast(&format!("Imported {} track(s){}{}", r.s("added"),
                            if r.truthy("duplicates_skipped") { format!(", skipped {} duplicate(s)", r.s("duplicates_skipped")) } else { String::new() },
                            if r.truthy("requested") { format!(", requested {}", r.s("requested")) } else { String::new() }), "ok");
                        m3.close();
                        app::navigate(&format!("/playlists/{}", r.s("id")));
                        if has_into { reload(); }
                    }
                    Err(err) => {
                        toast(&err.message, "err");
                        dom::set_disabled(&btn, false);
                        dom::set_html(&btn, &format!("{}Import", icon("download")));
                    }
                }
            });
        });
    }
}

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Playlists</h1><div class="small muted">Import from anywhere, merge without duplicates, and sync to Finamp / Plexamp.</div></div>
      <div class="row wrap">
        <button class="btn hidden" id="merge">{list}Merge selected</button>
        <button class="btn" id="new">{plus}New</button>
        <button class="btn primary" id="import">{download}Import</button>
      </div>
    </div>
    <div id="pls"></div>"#, list = icon("list"), plus = icon("plus"), download = icon("download")));
        let bx = dom::qs_in(&view, "#pls").expect("#pls");
        dom::on(&bx, "click", |e| {
            if dom::closest(e, "input").is_some() { return; }
            if let Some(t) = dom::closest(e, "[data-href]") { app::navigate(&dom::data(&t, "href")); }
        });
        {
            let (bx2, v2) = (bx.clone(), view.clone());
            dom::on(&bx, "change", move |_| {
                let n = dom::qsa_in(&bx2, "[data-merge]:checked").len();
                if let Some(b) = dom::qs_in(&v2, "#merge") {
                    dom::toggle_class(&b, "hidden", n < 2);
                    dom::set_html(&b, &format!("{}Merge {n} playlists", icon("list")));
                }
            });
        }
        dom::on_sel(&view, "#import", "click", |_| import_modal("", None));
        dom::on_sel(&view, "#new", "click", |_| dom::spawn(async {
            if let Ok(r) = api::post("/playlists", json!({ "name": "New playlist" })).await { app::navigate(&format!("/playlists/{}", r.s("id"))); }
        }));
        {
            let bx2 = bx.clone();
            dom::on_sel(&view, "#merge", "click", move |_| {
                let ids: Vec<Value> = dom::qsa_in(&bx2, "[data-merge]:checked").iter()
                    .map(|i| dom::data(i, "merge").parse::<i64>().map(Value::from).unwrap_or(Value::Null)).collect();
                let m = modal(ModalOpts::new("Merge playlists", &format!(r#"<p class="muted" style="margin-top:0">Tracks from {} playlists are combined into one. The same song + version is only kept once – remixes, live versions and covers stay separate.</p>
        <label class="field"><span>Name</span><input class="input" id="mname" value="Merged playlist"></label>
        <label class="switch mt"><input type="checkbox" id="mdel"><i></i>Delete the original playlists</label>"#, ids.len()))
                    .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Merge</button>"#));
                if let Some(ok) = m.qs("#ok") {
                    let m2 = m.clone();
                    dom::on(&ok, "click", move |_| {
                        let (m3, ids) = (m2.clone(), ids.clone());
                        dom::spawn(async move {
                            let Ok(r) = api::post("/playlists/merge", json!({ "ids": ids, "name": dom::value_of(&m3.el, "#mname"), "delete_sources": dom::checked_of(&m3.el, "#mdel") })).await else { return };
                            toast(&format!("Merged {} tracks → {} unique ({} duplicates removed)", r.s("total"), r.s("added"), r.s("duplicates_skipped")), "ok");
                            m3.close();
                            app::navigate(&format!("/playlists/{}", r.s("id")));
                        });
                    });
                }
            });
        }
        // load()
        let rows = api::get("/playlists").await.map_err(|e| e.message)?.items();
        dom::set_html(&bx, &if rows.is_empty() { empty_state(&icon("list"), "No playlists yet", "") } else { format!(r#"<div class="grid">{}</div>"#, rows.iter().map(pl_card).collect::<String>()) });
        Ok(())
    })
}

/// The page's `act(fn, msg)`: run the request, toast the message and re-render; toast the error otherwise.
async fn act(f: impl Future<Output = ApiResult>, msg: impl Fn(&Value) -> String) {
    match f.await {
        Ok(r) => { toast(&msg(&r), "ok"); reload(); }
        Err(e) => toast(&e.message, "err"),
    }
}

pub fn render_playlist(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let id = ctx.param(0);
        let pl_path = format!("/playlists/{id}");
        let (pl, conns) = futures::try_join!(api::get(&pl_path), api::get("/connections")).map_err(|e| e.message)?;
        let connected: Vec<String> = conns.o("connections").as_object().map(|o| o.keys().filter(|k| target_name(k).is_some()).cloned().collect()).unwrap_or_default();
        let items = pl.a("items");
        let missing = items.iter().filter(|i| requestable(i)).count();
        let liked = pl.s("kind") == "liked";
        let source = pl.s("source");
        let (total, matched) = (pl.f("total"), pl.f("matched"));
        let sync_targets = pl.a("sync_targets");
        let rows: String = items.iter().enumerate().map(|(i, it)| format!(r##"<tr data-item="{item}">
        <td class="num">{n}</td>
        <td><div style="font-weight:600">{title}</div>{title_sub}<div class="small muted">{artist}</div></td>
        <td class="hide-sm muted">{album}</td>
        <td>{status}</td>
        <td style="text-align:right;white-space:nowrap">{tmenu}<button class="btn sm ghost" data-rm title="Remove">{x}</button></td></tr>"##,
            item = it.s("id"), n = i + 1, title = esc(&crate::ui::tr(it, "title")), title_sub = crate::ui::tr_sub(it, "title"), artist = crate::ui::artist_links(it),
            album = if it.truthy("album_id") { format!(r##"<a href="#/library/album/{}">{}</a>"##, esc(&it.s("album_id")), esc(&it.s("album"))) } else { esc(&it.s("album")) },
            status = if it.truthy("track_id") {
                format!(r#"<span class="badge ok" title="{}">✓ in library</span>{}"#, esc(&it.s("match_reason")), match it.s("verify_status").as_str() {
                    "verified" => r#" <span class="badge ok" title="Audio fingerprint confirmed">verified</span>"#,
                    "mismatch" => r#" <span class="badge err">wrong audio</span>"#,
                    _ => "",
                })
            } else if let Some(s) = it.st("request_status") {
                match it.st("request_message") {
                    Some(m) => format!(r#"<span title="{}">{}</span>"#, esc(m), status_badge(s)),
                    None => status_badge(s),
                }
            } else { r#"<span class="badge">missing</span>"#.to_string() },
            x = icon("x"),
            // the song's ⋯ (Play next, Add to queue, Download for offline …) for rows in the library
            tmenu = if it.truthy("track_id") { format!(r#"<button class="btn sm ghost icon" data-ptmenu="{i}" aria-label="More" title="More">{}</button>"#, icon("more")) } else { String::new() },
        )).collect();
        dom::set_html(&view, &format!(r##"
    <div class="detail-head">
      <div class="cover">{cover}</div>
      <div class="grow">
        <div class="kicker">{kicker} · {src}</div>
        <h1 {h1attr}>{name}</h1>
        <div class="muted">{total} tracks · <b style="color:var(--ok)">{matched}</b> in library · {missing_n} missing</div>
        <div class="progress mt" style="max-width:360px"><i style="width:{pct}%"></i></div>
        <div class="row wrap mt">
          {play}
          {pl_menu}
          {req}
          <button class="btn" id="add">{plus}Add from link</button>
          <button class="btn" id="dedupe">{check}Remove duplicates</button>
          <button class="btn" id="rematch">{refresh}Re-check library</button>
          {pull}
          {del}
        </div>
      </div>
    </div>
    <div class="card row wrap between" style="margin-bottom:16px">
      <div class="row wrap">
        <b>Sync to</b>
        {sync_to}
        {liked_note}
      </div>
      <div class="row">
        {follow}
        {sync_btn}
      </div>
    </div>
    <div class="card flat"><div class="table-wrap"><table class="table">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Album</th><th>Status</th><th></th></tr></thead>
      <tbody>{rows}
      </tbody></table></div></div>
    {empty}"##,
            cover = if liked { r#"<div class="ph" style="background:var(--grad);color:#fff;font-size:80px">♥</div>"#.to_string() } else { art(&pl.s("cover_url"), &pl.s("name"), "") },
            kicker = if liked { "Liked songs · synced everywhere" } else { "Playlist" },
            src = esc(&src_label(&pl)),
            h1attr = if !liked { r#"contenteditable="true" spellcheck="false" id="pl-name" title="Click to rename""# } else { "" },
            name = esc(&pl.s("name")),
            total = pl.s("total"), matched = pl.s("matched"), missing_n = js_string(&json!(total - matched)),
            pct = if pl.truthy("total") { js_string(&json!(matched / total * 100.0)) } else { "0".to_string() },
            play = if pl.truthy("matched") { format!(r#"<button class="btn primary" id="pl-play">{}Play</button>"#, icon("play")) } else { String::new() },
            pl_menu = if pl.truthy("matched") && crate::pages::downloads::enabled() { format!(r#"<button class="btn icon" id="pl-menu" aria-label="More options" title="More options">{}</button>"#, icon("more")) } else { String::new() },
            req = if missing > 0 { format!(r#"<button class="btn {}" id="req">{}Download {missing} missing</button>"#, if pl.truthy("matched") { "" } else { "primary" }, icon("download")) } else { String::new() },
            plus = icon("plus"), check = icon("check"), refresh = icon("refresh"),
            pull = if pl.truthy("source_url") { format!(r#"<button class="btn" id="refresh">{}Pull from {}</button>"#, icon("refresh"), esc(src_name(&source).unwrap_or("source"))) } else { String::new() },
            del = if !liked { format!(r#"<button class="btn danger ghost" id="del">{}</button>"#, icon("trash")) } else { String::new() },
            sync_to = if !connected.is_empty() {
                connected.iter().filter(|s| **s != source).map(|s| format!(r#"<label class="switch"><input type="checkbox" data-target="{s}" {}><i></i>{}</label>"#,
                    if sync_targets.iter().any(|t| t.as_str() == Some(s)) { "checked" } else { "" }, esc(target_name(s).unwrap_or("")))).collect::<String>()
            } else {
                r##"<span class="muted small">Connect Jellyfin, Plex or Navidrome on the <a href="#/connections" style="color:var(--accent)">Connections</a> page.</span>"##.to_string()
            },
            liked_note = if liked { r#"<span class="small faint">Liked songs also sync as favourites / loves automatically.</span>"# } else { "" },
            follow = if pl.truthy("source_url") { format!(r#"<label class="switch"><input type="checkbox" id="follow" {}><i></i>Follow source</label>"#, if pl.truthy("follow") { "checked" } else { "" }) } else { String::new() },
            sync_btn = if !connected.is_empty() { format!(r#"<button class="btn sm" id="sync">{}Sync now</button>"#, icon("refresh")) } else { String::new() },
            rows = rows,
            empty = if items.is_empty() { empty_state(&icon("music"), "Empty playlist", "Add tracks from an album page or from a link.") } else { String::new() },
        ));
        let playable: Vec<Value> = items.iter().filter(|i| i.truthy("track_id")).map(as_track).collect();
        {
            let playable = playable.clone();
            dom::on_sel(&view, "#pl-menu", "click", move |e| {
                let Some(anchor) = dom::closest(e, "#pl-menu") else { return };
                let mut menu: Vec<Option<crate::ui::MenuItem>> = vec![];
                { let p = playable.clone(); menu.push(crate::ui::MenuItem::new("Play next", &icon("next"), move || { player::play_next(p.clone()); toast("Plays next", "ok"); })); }
                { let p = playable.clone(); menu.push(crate::ui::MenuItem::new("Add to queue", &icon("list"), move || { player::enqueue(p.clone()); toast("Added to queue", "ok"); })); }
                menu.extend(crate::pages::downloads::menu_items(&playable));
                crate::ui::pop_menu(&anchor, menu);
            });
        }
        {
            let rows = items.clone();
            dom::on_sel(&view, "tbody", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-ptmenu]") else { return };
                let Some(it) = dom::data(&b, "ptmenu").parse::<usize>().ok().and_then(|i| rows.get(i)) else { return };
                crate::pages::library::track_menu(&b, &as_track(it), app::is_admin(), Some(Rc::new(reload)));
            });
        }
        dom::on_sel(&view, "#pl-play", "click", move |_| player::play_queue(playable.clone(), 0, "list"));
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#req", "click", move |_| {
                let id3 = id2.clone();
                album_choice("Download missing songs", Rc::new(move |mode| {
                    let id4 = id3.clone();
                    dom::spawn(act(async move { api::post(&format!("/playlists/{id4}/request-missing"), json!({ "album_mode": mode })).await },
                        missing_report_text));
                }));
            });
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#dedupe", "click", move |_| {
                let id3 = id2.clone();
                dom::spawn(act(async move { api::post_empty(&format!("/playlists/{id3}/dedupe")).await },
                    |r| if r.truthy("removed") { format!("Removed {} duplicate(s)", r.s("removed")) } else { "No duplicates".to_string() }));
            });
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#rematch", "click", move |_| {
                let id3 = id2.clone();
                dom::spawn(act(async move { api::post_empty(&format!("/playlists/{id3}/rematch")).await }, |r| format!("{} newly matched", r.s("matched"))));
            });
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#refresh", "click", move |_| {
                let id3 = id2.clone();
                dom::spawn(act(async move { api::post_empty(&format!("/playlists/{id3}/refresh")).await }, |r| format!("{} new track(s)", r.s("added"))));
            });
        }
        {
            let into = id.parse::<i64>().unwrap_or(0);
            dom::on_sel(&view, "#add", "click", move |_| import_modal("", Some(into)));
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#sync", "click", move |_| {
                let id3 = id2.clone();
                dom::spawn(act(async move { api::post_empty(&format!("/playlists/{id3}/sync")).await }, |r| if r.truthy("errors") {
                    format!("Synced with {} error(s): {}", r.s("errors"), r.a("log").iter().map(js_string).collect::<Vec<_>>().join("; "))
                } else { "Synced".to_string() }));
            });
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "#follow", "change", move |e| {
                let checked = dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false);
                let id3 = id2.clone();
                dom::spawn(async move { let _ = api::patch(&format!("/playlists/{id3}"), json!({ "follow": checked })).await; });
            });
        }
        {
            let (v2, id2) = (view.clone(), id.clone());
            dom::on_each(&view, "[data-target]", "change", move |_, _| {
                let targets: Vec<String> = dom::qsa_in(&v2, "[data-target]:checked").iter().map(|c| dom::data(c, "target")).collect();
                let id3 = id2.clone();
                dom::spawn(async move {
                    if api::patch(&format!("/playlists/{id3}"), json!({ "sync_targets": targets })).await.is_ok() {
                        toast("Sync targets saved – syncing on the next pass (or press Sync now)", "ok");
                    }
                });
            });
        }
        if let Some(name) = dom::qs_in(&view, "#pl-name") {
            {
                let n2 = name.clone();
                dom::on(&name, "keydown", move |e| { if dom::key(e) == "Enter" { dom::prevent(e); if let Some(h) = dom::html_el(&n2) { let _ = h.blur(); } } });
            }
            {
                let (n2, id2, pl_name) = (name.clone(), id.clone(), pl.s("name"));
                dom::on(&name, "blur", move |_| {
                    let t = dom::text_of(&n2).trim().to_string();
                    if !t.is_empty() && t != pl_name {
                        let id3 = id2.clone();
                        dom::spawn(async move { let _ = api::patch(&format!("/playlists/{id3}"), json!({ "name": t })).await; });
                    }
                });
            }
        }
        {
            let (id2, pl_name) = (id.clone(), pl.s("name"));
            dom::on_sel(&view, "#del", "click", move |_| {
                let (id3, pl_name) = (id2.clone(), pl_name.clone());
                dom::spawn(async move {
                    if !confirm_dialog("Delete playlist", &format!("Delete “{pl_name}”? Songs stay in the library."), "Delete", true).await { return; }
                    if api::del(&format!("/playlists/{id3}")).await.is_ok() { app::navigate("/playlists"); }
                });
            });
        }
        {
            let id2 = id.clone();
            dom::on_sel(&view, "tbody", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-rm]") else { return };
                let Some(tr) = dom::closest_of(&b, "[data-item]") else { return };
                let id3 = id2.clone();
                dom::spawn(async move {
                    if api::del(&format!("/playlists/{id3}/items/{}", dom::data(&tr, "item"))).await.is_ok() { tr.remove(); }
                });
            });
        }
        Ok(())
    })
}

/// A playlist row that is in the library, as the player and the song menu want it.
fn as_track(i: &Value) -> Value {
    json!({
        "id": i.g("track_id"), "title": i.g("title"), "artist": i.g("artist"), "artists": i.g("artists"), "album": i.g("album"), "album_id": i.g("album_id"),
        "format": i.g("format"), "bitrate": i.g("bitrate"), "duration": i.g("duration"),
    })
}

pub fn album_choice(title: &str, go: Rc<dyn Fn(Option<String>)>) {
    let m = modal(ModalOpts::new(&esc(title), &format!(r#"<p class="muted" style="margin-top:0">When a song is part of an album:</p>
      <div class="stack">
        <button class="btn" data-m="song">{}Just the song</button>
        <button class="btn" data-m="album">{}Download the whole album (best quality available)</button>
        <button class="btn ghost" data-m="">Use the admin default</button>
      </div>"#, icon("music"), icon("disc"))));
    if let Some(stack) = m.qs(".stack") {
        let m2 = m.clone();
        dom::on(&stack, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-m]") else { return };
            m2.close();
            let v = dom::data(&b, "m");
            go(if v.is_empty() { None } else { Some(v) });
        });
    }
}

pub fn add_to_playlist_modal(track_ids: Vec<i64>) {
    dom::spawn(async move {
        let Ok(rows) = api::get("/playlists").await else { return };
        let list: String = rows.items().iter().map(|p| format!(r#"<button class="list-item btn ghost" style="justify-content:flex-start;border-radius:0" data-pl="{id}">
      {ic}<span class="grow" style="text-align:left">{name}</span><span class="small faint">{total}</span></button>"#,
            id = p.s("id"), ic = if p.s("kind") == "liked" { "♥".to_string() } else { icon("list") }, name = esc(&p.s("name")), total = p.s("total"))).collect();
        let m = modal(ModalOpts::new("Add to playlist", &format!(r#"<div class="list card flat">{list}</div>
      <div class="row mt"><input class="input" id="newpl" placeholder="…or a new playlist name"><button class="btn" id="mk">{}Create</button></div>"#, icon("plus"))));
        let track_ids = Rc::new(track_ids);
        let add: Rc<dyn Fn(String)> = {
            let (m2, ids) = (m.clone(), track_ids.clone());
            Rc::new(move |pid: String| {
                let (m3, ids) = (m2.clone(), ids.clone());
                dom::spawn(async move {
                    let Ok(r) = api::post(&format!("/playlists/{pid}/tracks"), json!({ "track_ids": *ids })).await else { return };
                    toast(&format!("Added {}{}", r.s("added"), if r.truthy("duplicates_skipped") { format!(" ({} already there)", r.s("duplicates_skipped")) } else { String::new() }), "ok");
                    m3.close();
                });
            })
        };
        {
            let add2 = add.clone();
            dom::on(&m.el, "click", move |e| { if let Some(b) = dom::closest(e, "[data-pl]") { add2(dom::data(&b, "pl")); } });
        }
        if let Some(mk) = m.qs("#mk") {
            let (m2, add2) = (m.clone(), add.clone());
            dom::on(&mk, "click", move |_| {
                let (m3, add3) = (m2.clone(), add2.clone());
                dom::spawn(async move {
                    let name = dom::value_of(&m3.el, "#newpl").trim().to_string();
                    let Ok(r) = api::post("/playlists", json!({ "name": if name.is_empty() { "New playlist".to_string() } else { name } })).await else { return };
                    add3(r.s("id"));
                });
            });
        }
    });
}

#[cfg(test)]
mod missing_report_tests {
    use super::*;

    #[test]
    fn the_toast_counts_and_explains() {
        assert_eq!(missing_report_text(&json!({ "requested": 120, "in_progress": 0, "failed": [] })), "Requested 120");
        assert_eq!(missing_report_text(&json!({ "requested": 120, "in_progress": 600, "failed": [] })), "Requested 120 · 600 already in progress");
        let failed: Vec<Value> = (0..64).map(|i| json!({ "title": format!("Song {i}"), "reason": if i % 2 == 0 { "Track needs a MusicBrainz id or artist + title" } else { "Weekly request limit reached (50). Ask an admin to raise it." } })).collect();
        assert_eq!(missing_report_text(&json!({ "requested": 120, "in_progress": 600, "failed": failed })),
            "Requested 120 · 600 already in progress · 64 couldn't be requested (Track needs a MusicBrainz id or artist + title; Weekly request limit reached (50). Ask an admin to raise it.)");
        // more than three different reasons are counted, not listed
        let failed: Vec<Value> = (0..5).map(|i| json!({ "title": "x", "reason": format!("reason {i}") })).collect();
        assert_eq!(missing_report_text(&json!({ "requested": 0, "failed": failed })), "Requested 0 · 5 couldn't be requested (reason 0; reason 1; reason 2; 2 more)");
        // an old server that only sends the count
        assert_eq!(missing_report_text(&json!({ "requested": 3 })), "Requested 3");
    }

    #[test]
    fn only_rows_whose_request_is_over_or_gone_are_asked_for() {
        assert!(requestable(&json!({ "title": "a" })));
        assert!(requestable(&json!({ "request_id": 5 })), "the request row is gone");
        for s in ["failed", "cancelled", "denied"] {
            assert!(requestable(&json!({ "request_id": 5, "request_status": s })), "{s}");
        }
        for s in ["pending", "queued", "searching", "downloading", "completed"] {
            assert!(!requestable(&json!({ "request_id": 5, "request_status": s })), "{s}");
        }
        assert!(!requestable(&json!({ "track_id": 9 })));
    }
}
