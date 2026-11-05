//! Settings → Auto-download: what Encore downloads on its own, how it picks quality, and for whom.
//!
//! A Leptos view ([`AutoDownload`]) drawn from GET /api/admin/autodownload. Save, Run now and Look for upgrades
//! now send the switches and boxes as they are on the page (read from it, as before).
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{json, J};
use crate::ui::{fmt, icon, toast};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::{Map, Value};
use web_sys::Element;

/// A switch bound to settings key `k`, on when `s[k]` is truthy, with its label in bold and a help line under it.
fn sw(s: &Value, k: &'static str, label: &'static str, help: &'static str) -> impl IntoView {
    let help = (!help.is_empty()).then(|| view! { <br/><span class="small muted">{help}</span> });
    view! {
        <label class="switch" style="align-items:flex-start"><input type="checkbox" data-k=k checked=s.truthy(k)/><i style="margin-top:2px"></i>
            <span><b>{label}</b>{help}</span></label>
    }
}

/// What Look for upgrades now did, in words: "Looking for lossless copies of 3 album(s) and hi-res copies of 5".
fn upgrade_text(r: &Value) -> String {
    // an older server answers only `queued` (lossless upgrades)
    let (lossless, hires) = if r.g("lossless").is_null() { (r.i("queued"), 0) } else { (r.i("lossless"), r.i("hires")) };
    match (lossless, hires) {
        (0, 0) => "Nothing to upgrade right now".to_string(),
        (l, 0) => format!("Looking for lossless copies of {l} album(s)"),
        (0, h) => format!("Looking for hi-res copies of {h} album(s)"),
        (l, h) => format!("Looking for lossless copies of {l} album(s) and hi-res copies of {h}"),
    }
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

/// Saves the rules as they are on the page, then posts to `then` (Run now, Look for upgrades now); a failure only
/// reaches the console, as before.
async fn save_then(root: &Element, then: &str) -> Option<Value> {
    if let Err(e) = api::put("/admin/autodownload", collect(root)).await {
        dom::console_error(&e.message);
        return None;
    }
    match api::post_empty(then).await {
        Ok(r) => Some(r),
        Err(e) => {
            dom::console_error(&e.message);
            None
        }
    }
}

/// The Auto-download page: the view, mounted once the answer has come (its failure is the page's error).
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let d = api::get("/admin/autodownload").await.map_err(|e| e.message)?;
        let album_upgrade = d.g("settings").s("album_upgrade");
        dom::set_html(&view, "");
        let root = view.clone();
        // kept while this page is on screen; the next page takes it off
        if let Some(m) = kit_leptos::rt::mount(&view, move || view! { <AutoDownload data=d root/> }) {
            dom::keep(m);
        }
        // the choice is set on the drawn list, as before (no option carries `selected`)
        if let Some(sel) = dom::qs_in(&view, r#"[data-k="album_upgrade"]"#) {
            dom::set_value(&sel, &album_upgrade);
        }
        Ok(())
    })
}

/// The page: its heading with Run now and Save, the counts, the two cards of rules and the users table.
#[component]
pub fn AutoDownload(
    /// The answer of GET /api/admin/autodownload: `settings`, `stats` and `users`.
    data: Value,
    /// The page's element: the buttons read the rules from it.
    root: Element,
) -> impl IntoView {
    let root = StoredValue::new_local(root);
    let s = data.g("settings").clone();
    let stats = data.g("stats");
    let users = data
        .a("users")
        .iter()
        .map(|u| {
            let admin = matches!(u.s("role").as_str(), "admin" | "owner").then(|| view! { <span class="badge">"admin"</span> });
            view! {
                <tr><td><b>{u.s("username")}</b>" "{admin}</td><td>{u.s("followed")}</td><td>{u.s("connections")}</td>
                    <td><label class="switch"><input type="checkbox" data-user=u.s("id") checked=u.truthy("auto_download")/><i></i></label></td></tr>
            }
        })
        .collect_view();
    let on_save = move |_| {
        dom::spawn(async move {
            match api::put("/admin/autodownload", collect(&root.get_value())).await {
                Ok(_) => toast("Auto-download rules saved", "ok"),
                Err(e) => toast(&e.message, "err"),
            }
        });
    };
    let on_run = move |_| {
        dom::spawn(async move {
            if save_then(&root.get_value(), "/admin/autodownload/run-now").await.is_some() {
                toast("Checking likes and followed playlists now…", "ok");
            }
        });
    };
    let on_upgrade = move |_| {
        dom::spawn(async move {
            if let Some(r) = save_then(&root.get_value(), "/admin/autodownload/upgrade-now").await {
                toast(&upgrade_text(&r), "ok");
            }
        });
    };
    let refresh = icon("refresh");
    view! {
        <div class="row wrap between" style="margin-bottom:18px">
            <div><h1>"Auto-download"</h1><div class="small muted">"Decide what Encore downloads on its own, how it picks quality, and for whom."</div></div>
            <div class="row"><button class="btn" id="run" on:click={on_run} {..html(format!("{refresh}Run now"))}>{""}</button><button class="btn primary" id="save" on:click={on_save} {..html(format!("{}Save", icon("check")))}>{""}</button></div>
        </div>
        <div class="stats" style="margin-bottom:20px">
            <div class="stat"><b>{fmt::num(stats.f("auto_active"))}</b><span>"Auto-downloads in progress"</span></div>
            <div class="stat"><b>{fmt::num(stats.f("album_waiting"))}</b><span>"Songs waiting on their album"</span></div>
            <div class="stat"><b>{fmt::num(stats.f("lossy_albums"))}</b><span>"Albums only in lossy quality"</span></div>
            <div class="stat"><b>{fmt::num(stats.f("cd_albums"))}</b><span>"Albums only in CD quality"</span></div>
            <div class="stat"><b>{fmt::num(stats.f("upgrades_done"))}</b><span>"Quality upgrades done"</span></div>
        </div>
        <div class="row wrap gap-lg" style="align-items:flex-start">
            <div class="card stack" style="flex:1;min-width:320px">
                <h2>"What gets downloaded automatically"</h2>
                {sw(&s, "auto_request_likes", "Liked songs", "A song liked in Finamp, Plexamp, YouTube Music, Spotify, Tidal, Deezer… that isn’t in the library yet gets downloaded.")}
                <label class="field"><span>"Auto-discover: from each user’s ListenBrainz "<i>"Weekly Exploration"</i>", download the top"</span>
                    <div class="row"><input class="input" type="number" min="0" max="50" data-k="discover_auto_count" value=s.s("discover_auto_count") style="width:110px"/><span class="muted">"new songs every week (0 = off)"</span></div></label>
                {sw(&s, "discover_catalog_merge", "Show releases MusicBrainz is missing", "Artist pages also list albums/EPs/singles found on Deezer and Apple Music that aren’t in MusicBrainz, so they can be requested.")}
                {sw(&s, "auto_request_followed", "New tracks in followed playlists", "Playlists set to “Follow source” pull new tracks every sync and download the missing ones.")}
                {sw(&s, "auto_approve", "Skip the approval queue for everyone", "Off: users’ manual requests wait for an admin. Auto-downloads for enabled users are always approved.")}
                <label class="field"><span>"Check for new likes / playlist tracks every"</span>
                    <div class="row"><input class="input" type="number" min="0" data-k="sync_interval_minutes" value=s.s("sync_interval_minutes") style="width:110px"/><span class="muted">"minutes (0 = never)"</span></div></label>
            </div>
            <div class="card stack" style="flex:1;min-width:320px">
                <h2>"Quality"</h2>
                <label class="field"><span>"When a song is part of an album"</span>
                    <select class="input" data-k="album_upgrade">
                        <option value="album_ep">"Download the whole album or EP it’s from (recommended)"</option>
                        <option value="album">"Download the whole album (studio albums only)"</option>
                        <option value="off">"Only download the song"</option>
                    </select>
                    <small>"Uses the official studio release from MusicBrainz – never compilations, live albums or soundtracks. If the album doesn’t contain that exact version (e.g. a remix), the song is fetched on its own."</small></label>
                {sw(&s, "quality_first", "Highest quality first", "Try every lossless source (Qobuz/Tidal/Deezer, then FLAC on Soulseek – 24-bit preferred) before torrents/Usenet, MP3s and YouTube Music.")}
                {sw(&s, "upgrade_lossy", "Keep upgrading lossy albums", "Periodically look for lossless copies of albums you only have as MP3/YouTube rips. Better files replace the old ones; playlists follow automatically.")}
                {sw(&s, "upgrade_hires", "Keep upgrading CD-quality albums to hi-res", "Periodically look on Qobuz/Tidal (streamrip, at Hi-res max) for 24-bit copies of albums you only have as 16-bit FLAC. A download that is CD quality again is not imported.")}
                <label class="field"><span>"Retry each lossy album every"</span>
                    <div class="row"><input class="input" type="number" min="1" data-k="upgrade_lossy_days" value=s.s("upgrade_lossy_days") style="width:110px"/><span class="muted">"days"</span>
                        <button class="btn sm" id="upg" on:click={on_upgrade} {..html(format!("{refresh}Look for upgrades now"))}>{""}</button></div></label>
                <div class="small muted">"Which sources are searched, and in what order: "<a href="#/settings?s=pipeline">"Settings → Downloads → Download pipeline"</a>"."</div>
            </div>
        </div>
        <div class="section-head"><h2>"Users"</h2><span class="small muted">"Who auto-downloads are enabled for"</span></div>
        <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>"User"</th><th>"Followed playlists"</th><th>"Connected accounts"</th><th>"Auto-download"</th></tr></thead><tbody>
            {users}
        </tbody></table></div></div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Look for upgrades now says what it started, also to an older server that only answers `queued`.
    #[test]
    fn the_upgrade_answer_reads_in_words() {
        assert_eq!(upgrade_text(&json!({ "lossless": 0, "hires": 0 })), "Nothing to upgrade right now");
        assert_eq!(upgrade_text(&json!({ "lossless": 3, "hires": 5 })), "Looking for lossless copies of 3 album(s) and hi-res copies of 5");
        assert_eq!(upgrade_text(&json!({ "queued": 2 })), "Looking for lossless copies of 2 album(s)");
    }
}
