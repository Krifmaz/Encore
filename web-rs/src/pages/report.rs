//! ⋯ → Report a problem (songs and albums), and Settings → Blocked: reports, blocked sources with Undo,
//! and the songs the quality checks flagged.
//!
//! Settings → Blocked is a Leptos view ([`Blocked`]) over one signal, the page's answers; each action loads them
//! again. The Report a problem dialog ([`report_modal`]) is still drawn as HTML.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, fmt, icon, modal, toast, ModalOpts};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::{json, Value};
use std::rc::Rc;
use web_sys::Element;

/// (key, label) of every reason, in menu order (server: encore_requests::report::REASONS).
pub const REASONS: [(&str, &str); 9] = [
    ("wrong_song", "Wrong song"),
    ("wrong_version", "Wrong version (live, remix, edit, missing parts)"),
    ("bad_audio", "Bad audio quality"),
    ("loudness", "Too quiet or too loud"),
    ("fake_lossless", "Fake lossless"),
    ("glitches", "Skips or glitches"),
    ("wrong_lyrics", "Wrong lyrics"),
    ("lyrics_sync", "Lyrics out of sync"),
    ("other", "Other"),
];

/// A reason about the lyrics, not the audio: the song stays, only its lyrics are replaced.
pub fn is_lyrics(reason: &str) -> bool {
    reason == "wrong_lyrics" || reason == "lyrics_sync"
}

/// The line under the title of the dialog: what Encore will do for the reason ticked now ("" for none yet).
pub fn explanation(kind: &str, title: &str, reason: &str) -> String {
    let what = if kind == "album" { "album" } else { "song" };
    match reason {
        "wrong_lyrics" => format!("The song itself stays as it is – nothing is deleted or downloaded again. Encore drops these lyrics for “{title}”, never picks them again for it and looks for other lyrics right away (time-synced first). If there are none, the song shows no lyrics rather than wrong ones."),
        "lyrics_sync" => format!("The song itself stays as it is – nothing is deleted or downloaded again. Encore drops this timing for “{title}” and looks for another time-synced version right away; if there is none, the same words are shown without timing."),
        _ => format!("What is wrong with “{title}”? Encore stops using the source this {what} came from, moves the file{} to the trash for 7 days and gets the {what} again from somewhere else. Plays, likes and playlists stay with the {what}.", if kind == "album" { "s" } else { "" }),
    }
}

/// The toast once the server answered a report.
pub fn done_toast(res: &Value) -> String {
    match res.s("status").as_str() {
        "replacing" => "Reported – looking for a better copy".to_string(),
        "lyrics_found" => "Reported – found other lyrics".to_string(),
        "lyrics_none" => "Reported – no other lyrics found, so none are shown".to_string(),
        _ if res.truthy("message") => res.s("message"),
        _ => "Reported".to_string(),
    }
}

/// The reason a quality-check flag suggests.
pub fn reason_for(codes: &[String]) -> &'static str {
    let has = |c: &str| codes.iter().any(|x| x == c);
    if has("fake_lossless") || has("upsampled") || has("low_bitrate") { return "fake_lossless"; }
    if has("quiet") { return "loudness"; }
    if has("wrong_length") { return "wrong_version"; }
    "bad_audio"
}

/// Report a problem with a song (kind "track") or an album (kind "album"). Encore blocks the source the
/// file came from, moves it to the trash and looks for another copy; a lyrics reason (songs only) keeps
/// the song and only replaces its lyrics. `reason` is ticked to start with ("" for none).
pub fn report_modal(kind: &str, id: Value, title: &str, reason: &str, on_done: Option<Rc<dyn Fn(Value)>>) {
    let reasons: String = REASONS.iter().filter(|(k, _)| kind != "album" || !is_lyrics(k))
        .map(|(k, l)| format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="radio" name="rp-reason" value="{k}" {}> {}</label>"#,
        if *k == reason { "checked" } else { "" }, esc(l))).collect();
    let body = format!(r#"<p class="small muted" style="margin-top:0" id="rp-what">{}</p>
      <div class="stack" style="gap:8px" id="rp-reasons">{reasons}</div>
      <label class="field mt"><span>Note</span><textarea class="input" id="rp-note" rows="2" placeholder="Optional – e.g. the intro is missing"></textarea></label>"#,
        esc(&explanation(kind, title, reason)));
    let m = modal(ModalOpts::new("Report a problem", &body).footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="rp-send">Report</button>"#));
    let Some(send) = m.qs("#rp-send") else { return };
    // the line above the reasons follows the one ticked: a lyrics reason keeps the song
    if let Some(list) = m.qs("#rp-reasons") {
        let (m2, kind, title) = (m.clone(), kind.to_string(), title.to_string());
        dom::on(&list, "change", move |_| {
            let r = m2.qs("input[name=rp-reason]:checked").map(|e| dom::value(&e)).unwrap_or_default();
            if let Some(p) = m2.qs("#rp-what") { dom::set_text(&p, &explanation(&kind, &title, &r)); }
        });
    }
    let (kind, send2) = (kind.to_string(), send.clone());
    dom::on(&send, "click", move |_| {
        let r = m.qs("input[name=rp-reason]:checked").map(|e| dom::value(&e));
        let note = m.qs("#rp-note").map(|e| dom::value(&e)).unwrap_or_default().trim().to_string();
        let Some(r) = r else { toast("Pick what is wrong", "err"); return };
        if r == "other" && note.is_empty() { toast("Say what is wrong", "err"); return; }
        dom::set_disabled(&send2, true);
        let mut body = json!({ "kind": kind, "reason": r, "note": note });
        if kind == "album" { body["album_id"] = id.clone(); } else { body["track_id"] = id.clone(); }
        let (m, send, on_done) = (m.clone(), send2.clone(), on_done.clone());
        dom::spawn(async move {
            match api::post("/report", body).await {
                Ok(res) => {
                    m.close();
                    toast(&done_toast(&res), "ok");
                    if let Some(f) = on_done { f(res); }
                }
                Err(e) => { toast(&e.message, "err"); dom::set_disabled(&send, false); }
            }
        });
    });
}

/// A text as one part of a link (encodeURIComponent).
fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// A report's status words and badge class.
fn status(s: &str) -> (&str, &str) {
    match s {
        "replacing" => ("Looking for a replacement", "warn"),
        "replaced" => ("Replaced", "ok"),
        "no_replacement" => ("No replacement yet", "err"),
        // lyrics reports (the song stays; only its lyrics are looked up again)
        "lyrics_searching" => ("Looking for other lyrics", "warn"),
        "lyrics_found" => ("New lyrics", "ok"),
        "lyrics_none" => ("No other lyrics yet", "err"),
        "dismissed" => ("Dismissed", ""),
        other => (other, ""),
    }
}

/// What Settings → Blocked draws from. The admin's parts are empty for a member.
#[derive(Clone)]
pub struct Loaded {
    /// Whether the reader is an admin (who sees everyone's reports, the blocks and the flagged songs).
    pub admin: bool,
    /// The answer of GET /api/admin/settings (admins).
    pub values: Value,
    /// The answer of GET /api/reports.
    pub reports: Value,
    /// The answer of GET /api/blocks (admins).
    pub blocks: Value,
    /// The answer of GET /api/quality/suspect (admins).
    pub suspect: Value,
}

/// Loads everything the page shows (a member only gets the reports).
async fn fetch(admin: bool) -> Result<Loaded, String> {
    let values = if admin { api::get("/admin/settings").await.map_err(|e| e.message)? } else { json!({}) };
    let reports = api::get("/reports").await.map_err(|e| e.message)?;
    let blocks = if admin { api::get("/blocks").await.map_err(|e| e.message)? } else { json!({ "blocks": [] }) };
    let suspect = if admin { api::get("/quality/suspect").await.map_err(|e| e.message)? } else { json!({ "tracks": [] }) };
    Ok(Loaded { admin, values, reports, blocks, suspect })
}

/// Loads the page again into `state`; a failure keeps what is shown (it only reaches the console, as before).
fn reload(state: RwSignal<Option<Loaded>>) {
    let Some(admin) = state.with_untracked(|s| s.as_ref().map(|s| s.admin)) else { return };
    dom::spawn(async move {
        match fetch(admin).await {
            Ok(l) => state.set(Some(l)),
            Err(e) => dom::console_error(&e),
        }
    });
}

/// Settings → Blocked: the view, mounted once the answers have come (a failure is the page's error).
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let role = ctx.user.s("role");
        let loaded = fetch(role == "admin" || role == "owner").await?;
        dom::set_html(&view, "");
        // the signal lives as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let state = owner.with(|| RwSignal::new(Some(loaded)));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Blocked state/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        Ok(())
    })
}

/// A button's action on the page: disables it, runs `act`, then loads the page again; on a failure the error
/// is a toast and the button is enabled again.
fn act<F>(state: RwSignal<Option<Loaded>>, act: impl Fn() -> F + 'static) -> impl Fn(web_sys::MouseEvent) + 'static
where
    F: std::future::Future<Output = Result<(), api::ApiError>> + 'static,
{
    move |e| {
        let b = dom::target_el(&e).and_then(|t| t.closest("button").ok().flatten());
        if let Some(b) = &b {
            dom::set_disabled(b, true);
        }
        let run = act();
        dom::spawn(async move {
            match run.await {
                Ok(()) => reload(state),
                Err(e) => {
                    toast(&e.message, "err");
                    if let Some(b) = &b {
                        dom::set_disabled(b, false);
                    }
                }
            }
        });
    }
}

/// One report: what, why, who and when, its status, Try again and (admins) Dismiss.
fn report_row(r: &Value, admin: bool, state: RwSignal<Option<Loaded>>) -> impl IntoView {
    let st = r.s("status");
    let (label, cls) = status(&st);
    let id = r.i("id");
    let name = format!("{} – {}", r.s("artist"), r.s("title"));
    let title = if r.s("kind") == "album" && r.truthy("album_id") {
        view! { <a href=format!("#/library/album/{}", enc(&r.s("album_id")))>{name}</a> }.into_any()
    } else {
        name.into_any()
    };
    let retry = (st == "no_replacement" || st == "lyrics_none").then(|| {
        let on = act(state, move || async move {
            api::post_empty(&format!("/reports/{id}/retry")).await?;
            toast("Trying again", "ok");
            Ok(())
        });
        view! { <button class="btn sm" data-retry=id on:click={on} {..html(format!("{}Try again", icon("refresh")))}>{""}</button> }
    });
    let dismiss = (admin && ["replacing", "no_replacement", "lyrics_found", "lyrics_none"].contains(&st.as_str())).then(|| {
        let on = act(state, move || async move {
            api::post_empty(&format!("/reports/{id}/dismiss")).await?;
            toast("Dismissed", "ok");
            Ok(())
        });
        view! { <button class="btn sm ghost" data-dismiss=id on:click=on>"Dismiss"</button> }
    });
    let reason = if r.truthy("reason_label") { r.s("reason_label") } else { r.s("reason") };
    let note = if r.truthy("note") { format!(" · “{}”", r.s("note")) } else { String::new() };
    let who = if admin && r.truthy("username") { format!("{} · ", r.s("username")) } else { String::new() };
    let src = if r.truthy("source") { format!(" · from {}", r.s("source")) } else { String::new() };
    let msg = if r.truthy("message") { format!(" · {}", r.s("message")) } else { String::new() };
    view! {
        <div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:10px" data-report=id>
            <div class="grow" style="min-width:220px"><div style="font-weight:650">{title}" "<span class="small muted">{if r.s("kind") == "album" { "album" } else { "song" }}</span></div>
                <div class="small">{format!("{reason}{note}")}</div>
                <div class="small faint">{format!("{who}{}{src}{msg}", fmt::ago(r.f("created_at")))}</div></div>
            <span class=format!("badge {cls}")>{label.to_string()}</span>
            <span class="row" style="gap:6px">{retry}{dismiss}</span>
        </div>
    }
}

/// One blocked source: what it is, why and when, with Undo.
fn block_row(b: &Value, state: RwSignal<Option<Loaded>>) -> impl IntoView {
    let id = b.i("id");
    let label = if b.truthy("label") { b.s("label") } else { b.s("key") };
    let reason = if b.truthy("reason_label") { format!(" · {}", b.s("reason_label")) } else { String::new() };
    let title = if b.truthy("title") { format!(" · {} – {}", b.s("artist"), b.s("title")) } else { String::new() };
    let who = if b.truthy("username") { format!(" · {}", b.s("username")) } else { String::new() };
    let undo = act(state, move || async move {
        api::del(&format!("/blocks/{id}")).await?;
        toast("Unblocked – that source may be used again", "ok");
        Ok(())
    });
    view! {
        <div class="list-item" style="flex-wrap:wrap;gap:10px" data-block=id>
            <div class="grow" style="min-width:220px"><div style="font-weight:600" class="ellipsis">{label}</div>
                <div class="small faint">{format!("{}{reason}{title}{who} · {}", b.s("kind"), fmt::ago(b.f("created_at")))}</div></div>
            <button class="btn sm" data-undo=id on:click=undo>"Undo"</button></div>
    }
}

/// One song the quality checks flagged: what was found, with Check again and Replace (which opens Report a
/// problem with the reason the flag suggests).
fn suspect_row(i: usize, t: &Value, state: RwSignal<Option<Loaded>>) -> impl IntoView {
    let tid = t.i("id");
    let check = act(state, move || async move {
        let q = api::post_empty(&format!("/quality/tracks/{tid}/check")).await?;
        let bad = q.s("status") == "suspect";
        toast(&if bad { q.s("summary") } else { "Looks fine now".to_string() }, if bad { "err" } else { "ok" });
        Ok(())
    });
    let t2 = t.clone();
    let replace = move |_| {
        let codes: Vec<String> = t2.a("issues").iter().map(|x| x.s("code")).collect();
        report_modal("track", t2.g("id").clone(), &t2.s("title"), reason_for(&codes), Some(Rc::new(move |_| reload(state))));
    };
    let br = if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() };
    let album = if t.truthy("album") { format!(" · {}", t.s("album")) } else { String::new() };
    view! {
        <div class="list-item" style="flex-wrap:wrap;gap:10px">
            <div class="grow" style="min-width:220px"><div style="font-weight:600"><a href=format!("#/library/album/{}", enc(&t.s("album_id")))>{format!("{} – {}", t.s("artist"), t.s("title"))}</a></div>
                <div class="small" style="color:var(--warn)">{t.s("summary")}</div>
                <div class="small faint">{format!("{}{br}{album}", t.s("format").to_uppercase())}</div></div>
            <span class="row" style="gap:6px"><button class="btn sm ghost" data-check=i on:click={check} {..html(format!("{}Check again", icon("refresh")))}>{""}</button><button class="btn sm primary" data-replace=i on:click=replace>"Replace"</button></span></div>
    }
}

/// A card's list, or its line for when it is empty.
fn list_or<V: IntoView>(rows: Vec<V>, empty: &'static str) -> AnyView {
    if rows.is_empty() {
        view! { <p class="muted small" style="margin:0">{empty}</p> }.into_any()
    } else {
        view! { <div class="list">{rows}</div> }.into_any()
    }
}

/// Settings → Blocked: what blocking does (with the import check for admins), the reports, and for admins the
/// blocked sources and the flagged songs.
#[component]
pub fn Blocked(
    /// What the page draws from (always `Some` once mounted).
    state: RwSignal<Option<Loaded>>,
) -> impl IntoView {
    move || {
        let Loaded { admin, values, reports, blocks, suspect } = state.get()?;
        let toggle = admin.then(|| {
            let locked = values.o("_meta").o("quality_check_on_import").s("source") == "env";
            let on_change = move |e: web_sys::Event| {
                let on = dom::target_el(&e).is_some_and(|t| dom::checked(&t));
                dom::spawn(async move {
                    match api::put("/admin/settings", json!({ "quality_check_on_import": on })).await {
                        Ok(_) => toast("Saved", "ok"),
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            };
            view! {
                <div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" id="q-import" checked=values.truthy("quality_check_on_import") disabled=locked on:change=on_change/><i></i>"Check every download before it enters the library"</label>
                    <small>"Fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet audio and the wrong length are turned away and the next copy is tried. The Quality scan under Scheduled tasks checks the songs already in the library."</small></div>
            }
        });
        let report_rows: Vec<_> = reports.a("reports").iter().map(|r| report_row(r, admin, state)).collect();
        let admin_cards = admin.then(|| {
            let block_rows: Vec<_> = blocks.a("blocks").iter().map(|b| block_row(b, state)).collect();
            let suspect_rows: Vec<_> = suspect.a("tracks").iter().enumerate().map(|(i, t)| suspect_row(i, t, state)).collect();
            view! {
                <div class="card stack" style="margin-bottom:16px">
                    <h2>"Blocked sources"</h2>
                    {list_or(block_rows, "No blocked sources.")}
                </div>
                <div class="card stack">
                    <h2>"Suspect quality"</h2>
                    <p class="muted small" style="margin:0">"Songs the quality checks flagged. Replace one to block where it came from and get it again."</p>
                    {list_or(suspect_rows, "Nothing flagged.")}
                </div>
            }
        });
        Some(view! {
            <div class="card stack" style="margin-bottom:16px">
                <h2>"Blocked"</h2>
                <p class="muted small" style="margin:0">"When someone reports a song or album (⋯ → Report a problem), the exact source it came from is blocked – that Soulseek file or folder, torrent, Usenet post, streaming item or YouTube video – and the music is fetched again from somewhere else. Undo a block to allow that source again."</p>
                {toggle}
            </div>
            <div class="card stack" style="margin-bottom:16px">
                <h2>"Reports"</h2>
                {list_or(report_rows, "Nothing reported yet.")}
            </div>
            {admin_cards}
        })
    }
}
