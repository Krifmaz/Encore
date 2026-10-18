//! Port of web/js/pages/autodownload.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::ui::{self, esc, icon, toast};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

/// The `sw(k, label, help)` helper: a switch bound to settings key `k`, checked when `s[k]` is truthy.
fn sw(s: &Value, k: &str, label: &str, help: &str) -> String {
    let help_html = if help.is_empty() { String::new() } else { format!(r#"<br><span class="small muted">{help}</span>"#) };
    format!(r#"<label class="switch" style="align-items:flex-start"><input type="checkbox" data-k="{k}" {checked}><i style="margin-top:2px"></i>
    <span><b>{label}</b>{help_html}</span></label>"#,
        checked = if s.truthy(k) { "checked" } else { "" })
}

/// `collect()`: `{ settings: { [data-k]: checked | value }, users: { [data-user]: checked } }`.
fn collect(view: &Element) -> Value {
    let mut settings = Map::new();
    for el in dom::qsa_in(view, "[data-k]") {
        let v = if el.get_attribute("type").as_deref() == Some("checkbox") { Value::Bool(dom::checked(&el)) } else { Value::String(dom::value(&el)) };
        settings.insert(dom::data(&el, "k"), v);
    }
    let mut users = Map::new();
    for el in dom::qsa_in(view, "[data-user]") {
        users.insert(dom::data(&el, "user"), Value::Bool(dom::checked(&el)));
    }
    json!({ "settings": settings, "users": users })
}

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let d = api::get("/admin/autodownload").await.map_err(|e| e.message)?;
        let s = d.g("settings").clone();
        let stats = d.g("stats");
        let users: String = d.a("users").iter().map(|u| format!(r#"<tr><td><b>{}</b> {}</td><td>{}</td><td>{}</td>
        <td><label class="switch"><input type="checkbox" data-user="{}" {}><i></i></label></td></tr>"#,
            esc(&u.s("username")),
            if matches!(u.s("role").as_str(), "admin" | "owner") { r#"<span class="badge">admin</span>"# } else { "" },
            u.s("followed"), u.s("connections"), u.s("id"),
            if u.truthy("auto_download") { "checked" } else { "" })).collect();
        dom::set_html(&view, &format!(r##"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Auto-download</h1><div class="small muted">Decide what Encore downloads on its own, how it picks quality, and for whom.</div></div>
      <div class="row"><button class="btn" id="run">{refresh}Run now</button><button class="btn primary" id="save">{check}Save</button></div>
    </div>
    <div class="stats" style="margin-bottom:20px">
      <div class="stat"><b>{auto_active}</b><span>Auto-downloads in progress</span></div>
      <div class="stat"><b>{album_waiting}</b><span>Songs waiting on their album</span></div>
      <div class="stat"><b>{lossy_albums}</b><span>Albums only in lossy quality</span></div>
      <div class="stat"><b>{upgrades_done}</b><span>Upgraded to lossless</span></div>
    </div>
    <div class="row wrap gap-lg" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:320px">
        <h2>What gets downloaded automatically</h2>
        {sw_likes}
        <label class="field"><span>Auto-discover: from each user’s ListenBrainz <i>Weekly Exploration</i>, download the top</span>
          <div class="row"><input class="input" type="number" min="0" max="50" data-k="discover_auto_count" value="{discover_auto_count}" style="width:110px"><span class="muted">new songs every week (0 = off)</span></div></label>
        {sw_catalog_merge}
        {sw_followed}
        {sw_approve}
        <label class="field"><span>Check for new likes / playlist tracks every</span>
          <div class="row"><input class="input" type="number" min="0" data-k="sync_interval_minutes" value="{sync_interval_minutes}" style="width:110px"><span class="muted">minutes (0 = never)</span></div></label>
      </div>
      <div class="card stack" style="flex:1;min-width:320px">
        <h2>Quality</h2>
        <label class="field"><span>When a song is part of an album</span>
          <select class="input" data-k="album_upgrade">
            <option value="album_ep">Download the whole album or EP it’s from (recommended)</option>
            <option value="album">Download the whole album (studio albums only)</option>
            <option value="off">Only download the song</option>
          </select>
          <small>Uses the official studio release from MusicBrainz – never compilations, live albums or soundtracks. If the album doesn’t contain that exact version (e.g. a remix), the song is fetched on its own.</small></label>
        {sw_quality_first}
        {sw_upgrade_lossy}
        <label class="field"><span>Retry each lossy album every</span>
          <div class="row"><input class="input" type="number" min="1" data-k="upgrade_lossy_days" value="{upgrade_lossy_days}" style="width:110px"><span class="muted">days</span>
          <button class="btn sm" id="upg">{refresh}Look for upgrades now</button></div></label>
        <div class="small muted">Which sources are searched, and in what order: <a href="#/settings?s=pipeline">Settings → Apps & sources → Download pipeline</a>.</div>
      </div>
    </div>
    <div class="section-head"><h2>Users</h2><span class="small muted">Who auto-downloads are enabled for</span></div>
    <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>User</th><th>Followed playlists</th><th>Connected accounts</th><th>Auto-download</th></tr></thead><tbody>
      {users}
    </tbody></table></div></div>"##,
            refresh = icon("refresh"), check = icon("check"),
            auto_active = ui::fmt::num(stats.f("auto_active")),
            album_waiting = ui::fmt::num(stats.f("album_waiting")),
            lossy_albums = ui::fmt::num(stats.f("lossy_albums")),
            upgrades_done = ui::fmt::num(stats.f("upgrades_done")),
            sw_likes = sw(&s, "auto_request_likes", "Liked songs", "A song liked in Finamp, Plexamp, YouTube Music, Spotify, Tidal, Deezer… that isn’t in the library yet gets downloaded."),
            discover_auto_count = esc(&s.s("discover_auto_count")),
            sw_catalog_merge = sw(&s, "discover_catalog_merge", "Show releases MusicBrainz is missing", "Artist pages also list albums/EPs/singles found on Deezer and Apple Music that aren’t in MusicBrainz, so they can be requested."),
            sw_followed = sw(&s, "auto_request_followed", "New tracks in followed playlists", "Playlists set to “Follow source” pull new tracks every sync and download the missing ones."),
            sw_approve = sw(&s, "auto_approve", "Skip the approval queue for everyone", "Off: users’ manual requests wait for an admin. Auto-downloads for enabled users are always approved."),
            sync_interval_minutes = esc(&s.s("sync_interval_minutes")),
            sw_quality_first = sw(&s, "quality_first", "Highest quality first", "Try every lossless source (Qobuz/Tidal/Deezer, then FLAC on Soulseek – 24-bit preferred) before torrents/Usenet, MP3s and YouTube Music."),
            sw_upgrade_lossy = sw(&s, "upgrade_lossy", "Keep upgrading lossy albums", "Periodically look for lossless copies of albums you only have as MP3/YouTube rips. Better files replace the old ones; playlists follow automatically."),
            upgrade_lossy_days = esc(&s.s("upgrade_lossy_days")),
            users = users,
        ));
        if let Some(sel) = dom::qs_in(&view, r#"[data-k="album_upgrade"]"#) { dom::set_value(&sel, &s.s("album_upgrade")); }
        // `let d` in the JS: the save handler keeps the server's answer
        let d = Rc::new(RefCell::new(d));
        {
            let (v, d) = (view.clone(), d.clone());
            dom::on_sel(&view, "#save", "click", move |_| {
                let (v, d) = (v.clone(), d.clone());
                dom::spawn(async move {
                    match api::put("/admin/autodownload", collect(&v)).await {
                        Ok(r) => { *d.borrow_mut() = r; toast("Auto-download rules saved", "ok"); }
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            });
        }
        {
            let v = view.clone();
            dom::on_sel(&view, "#run", "click", move |_| {
                let v = v.clone();
                dom::spawn(async move {
                    // no try/catch in the JS: a failure only reaches the console
                    if let Err(e) = api::put("/admin/autodownload", collect(&v)).await { dom::console_error(&e.message); return; }
                    if let Err(e) = api::post_empty("/admin/autodownload/run-now").await { dom::console_error(&e.message); return; }
                    toast("Checking likes and followed playlists now…", "ok");
                });
            });
        }
        {
            let v = view.clone();
            dom::on_sel(&view, "#upg", "click", move |_| {
                let v = v.clone();
                dom::spawn(async move {
                    if let Err(e) = api::put("/admin/autodownload", collect(&v)).await { dom::console_error(&e.message); return; }
                    match api::post_empty("/admin/autodownload/upgrade-now").await {
                        Ok(r) => toast(&if r.truthy("queued") { format!("Looking for lossless copies of {} album(s)", r.s("queued")) } else { "Nothing to upgrade right now".to_string() }, "ok"),
                        Err(e) => dom::console_error(&e.message),
                    }
                });
            });
        }
        Ok(())
    })
}
