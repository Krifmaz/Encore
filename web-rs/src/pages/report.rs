//! Port of web/js/pages/report.js – see docs/archive/ui-rewrite/CONTRACT.md
//! ⋯ → Report a problem (songs and albums), and Settings → Blocked: reports, blocked sources with Undo,
//! and the songs the quality checks flagged.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, fmt, icon, modal, toast, ModalOpts};
use serde_json::{json, Value};
use std::rc::Rc;
use web_sys::Element;

pub const REASONS: [(&str, &str); 7] = [
    ("wrong_song", "Wrong song"),
    ("wrong_version", "Wrong version (live, remix, edit, missing parts)"),
    ("bad_audio", "Bad audio quality"),
    ("loudness", "Too quiet or too loud"),
    ("fake_lossless", "Fake lossless"),
    ("glitches", "Skips or glitches"),
    ("other", "Other"),
];

/// The reason a quality-check flag suggests.
pub fn reason_for(codes: &[String]) -> &'static str {
    let has = |c: &str| codes.iter().any(|x| x == c);
    if has("fake_lossless") || has("upsampled") || has("low_bitrate") { return "fake_lossless"; }
    if has("quiet") { return "loudness"; }
    if has("wrong_length") { return "wrong_version"; }
    "bad_audio"
}

/// Report a problem with a song (kind "track") or an album (kind "album"). Encore blocks the source the
/// file came from, moves it to the trash and looks for another copy.
pub fn report_modal(kind: &str, id: Value, title: &str, reason: &str, on_done: Option<Rc<dyn Fn(Value)>>) {
    let what = if kind == "album" { "album" } else { "song" };
    let reasons: String = REASONS.iter().map(|(k, l)| format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="radio" name="rp-reason" value="{k}" {}> {}</label>"#,
        if *k == reason { "checked" } else { "" }, esc(l))).collect();
    let body = format!(r#"<p class="small muted" style="margin-top:0">What is wrong with “{}”? Encore stops using the source this {what} came from, moves the file{} to the trash for 7 days and gets the {what} again from somewhere else. Plays, likes and playlists stay with the {what}.</p>
      <div class="stack" style="gap:8px" id="rp-reasons">{reasons}</div>
      <label class="field mt"><span>Note</span><textarea class="input" id="rp-note" rows="2" placeholder="Optional – e.g. the intro is missing"></textarea></label>"#,
        esc(title), if kind == "album" { "s" } else { "" });
    let m = modal(ModalOpts::new("Report a problem", &body).footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="rp-send">Report</button>"#));
    let Some(send) = m.qs("#rp-send") else { return };
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
                    toast(&if res.s("status") == "replacing" { "Reported – looking for a better copy".to_string() } else if res.truthy("message") { res.s("message") } else { "Reported".to_string() }, "ok");
                    if let Some(f) = on_done { f(res); }
                }
                Err(e) => { toast(&e.message, "err"); dom::set_disabled(&send, false); }
            }
        });
    });
}

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

fn status(s: &str) -> (&str, &str) {
    match s {
        "replacing" => ("Looking for a replacement", "warn"),
        "replaced" => ("Replaced", "ok"),
        "no_replacement" => ("No replacement yet", "err"),
        "dismissed" => ("Dismissed", ""),
        other => (other, ""),
    }
}

fn report_row(r: &Value, admin: bool) -> String {
    let st = r.s("status");
    let (label, cls) = status(&st);
    let href = if r.s("kind") == "album" && r.truthy("album_id") { format!("#/library/album/{}", enc(&r.s("album_id"))) } else { String::new() };
    let name = format!("{} – {}", esc(&r.s("artist")), esc(&r.s("title")));
    let retry = if st == "no_replacement" { format!(r#"<button class="btn sm" data-retry="{}">{}Try again</button>"#, r.i("id"), icon("refresh")) } else { String::new() };
    let dismiss = if admin && (st == "replacing" || st == "no_replacement") { format!(r#"<button class="btn sm ghost" data-dismiss="{}">Dismiss</button>"#, r.i("id")) } else { String::new() };
    format!(r#"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:10px" data-report="{id}">
    <div class="grow" style="min-width:220px"><div style="font-weight:650">{title} <span class="small muted">{kind}</span></div>
      <div class="small">{reason}{note}</div>
      <div class="small faint">{who}{ago}{src}{msg}</div></div>
    <span class="badge {cls}">{label}</span>
    <span class="row" style="gap:6px">{retry}{dismiss}</span>
  </div>"#,
        id = r.i("id"),
        title = if href.is_empty() { name } else { format!(r#"<a href="{href}">{name}</a>"#) },
        kind = if r.s("kind") == "album" { "album" } else { "song" },
        reason = esc(&if r.truthy("reason_label") { r.s("reason_label") } else { r.s("reason") }),
        note = if r.truthy("note") { format!(" · “{}”", esc(&r.s("note"))) } else { String::new() },
        who = if admin && r.truthy("username") { format!("{} · ", esc(&r.s("username"))) } else { String::new() },
        ago = fmt::ago(r.f("created_at")),
        src = if r.truthy("source") { format!(" · from {}", esc(&r.s("source"))) } else { String::new() },
        msg = if r.truthy("message") { format!(" · {}", esc(&r.s("message"))) } else { String::new() },
        label = esc(label))
}

fn rerender(view: Element, ctx: Ctx) {
    dom::spawn(async move { if let Err(e) = render(view, ctx).await { dom::console_error(&e); } });
}

/// Settings → Blocked
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let role = ctx.user.s("role");
        let admin = role == "admin" || role == "owner";
        let values = if admin { api::get("/admin/settings").await.map_err(|e| e.message)? } else { json!({}) };
        let reps = api::get("/reports").await.map_err(|e| e.message)?;
        let blocks = if admin { api::get("/blocks").await.map_err(|e| e.message)? } else { json!({ "blocks": [] }) };
        let sus = if admin { api::get("/quality/suspect").await.map_err(|e| e.message)? } else { json!({ "tracks": [] }) };
        let meta = values.o("_meta");
        let locked_q = meta.o("quality_check_on_import").s("source") == "env";
        let toggle = if admin {
            format!(r#"<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" id="q-import" {} {}><i></i>Check every download before it enters the library</label>
        <small>Fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet audio and the wrong length are turned away and the next copy is tried. The Quality scan under Scheduled tasks checks the songs already in the library.</small></div>"#,
                if values.truthy("quality_check_on_import") { "checked" } else { "" }, if locked_q { "disabled" } else { "" })
        } else { String::new() };
        let rl = reps.a("reports");
        let reports = if rl.is_empty() { r#"<p class="muted small" style="margin:0">Nothing reported yet.</p>"#.to_string() }
            else { format!(r#"<div class="list">{}</div>"#, rl.iter().map(|r| report_row(r, admin)).collect::<String>()) };
        let tracks = sus.a("tracks");
        let admin_cards = if admin {
            let bl = blocks.a("blocks");
            let blist = if bl.is_empty() { r#"<p class="muted small" style="margin:0">No blocked sources.</p>"#.to_string() } else {
                format!(r#"<div class="list">{}</div>"#, bl.iter().map(|b| format!(r#"<div class="list-item" style="flex-wrap:wrap;gap:10px" data-block="{id}">
        <div class="grow" style="min-width:220px"><div style="font-weight:600" class="ellipsis">{label}</div>
          <div class="small faint">{kind}{reason}{title}{who} · {ago}</div></div>
        <button class="btn sm" data-undo="{id}">Undo</button></div>"#,
                    id = b.i("id"), label = esc(&if b.truthy("label") { b.s("label") } else { b.s("key") }), kind = esc(&b.s("kind")),
                    reason = if b.truthy("reason_label") { format!(" · {}", esc(&b.s("reason_label"))) } else { String::new() },
                    title = if b.truthy("title") { format!(" · {} – {}", esc(&b.s("artist")), esc(&b.s("title"))) } else { String::new() },
                    who = if b.truthy("username") { format!(" · {}", esc(&b.s("username"))) } else { String::new() },
                    ago = fmt::ago(b.f("created_at")))).collect::<String>())
            };
            let slist = if tracks.is_empty() { r#"<p class="muted small" style="margin:0">Nothing flagged.</p>"#.to_string() } else {
                format!(r#"<div class="list">{}</div>"#, tracks.iter().enumerate().map(|(i, t)| format!(r##"<div class="list-item" style="flex-wrap:wrap;gap:10px">
        <div class="grow" style="min-width:220px"><div style="font-weight:600"><a href="#/library/album/{aid}">{artist} – {title}</a></div>
          <div class="small" style="color:var(--warn)">{summary}</div>
          <div class="small faint">{fmt}{br}{album}</div></div>
        <span class="row" style="gap:6px"><button class="btn sm ghost" data-check="{i}">{refresh}Check again</button><button class="btn sm primary" data-replace="{i}">Replace</button></span></div>"##,
                    aid = enc(&t.s("album_id")), artist = esc(&t.s("artist")), title = esc(&t.s("title")), summary = esc(&t.s("summary")),
                    fmt = esc(&t.s("format").to_uppercase()), br = if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() },
                    album = if t.truthy("album") { format!(" · {}", esc(&t.s("album"))) } else { String::new() }, refresh = icon("refresh"))).collect::<String>())
            };
            format!(r#"<div class="card stack" style="margin-bottom:16px">
      <h2>Blocked sources</h2>
      {blist}
    </div>
    <div class="card stack">
      <h2>Suspect quality</h2>
      <p class="muted small" style="margin:0">Songs the quality checks flagged. Replace one to block where it came from and get it again.</p>
      {slist}
    </div>"#)
        } else { String::new() };
        dom::set_html(&view, &format!(r#"
    <div class="card stack" style="margin-bottom:16px">
      <h2>Blocked</h2>
      <p class="muted small" style="margin:0">When someone reports a song or album (⋯ → Report a problem), the exact source it came from is blocked – that Soulseek file or folder, torrent, Usenet post, streaming item or YouTube video – and the music is fetched again from somewhere else. Undo a block to allow that source again.</p>
      {toggle}
    </div>
    <div class="card stack" style="margin-bottom:16px">
      <h2>Reports</h2>
      {reports}
    </div>
    {admin_cards}"#));

        if let Some(q) = dom::qs_in(&view, "#q-import") {
            let q2 = q.clone();
            dom::on(&q, "change", move |_| {
                let on = dom::checked(&q2);
                dom::spawn(async move {
                    match api::put("/admin/settings", json!({ "quality_check_on_import": on })).await {
                        Ok(_) => toast("Saved", "ok"),
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            });
        }
        let tracks = Rc::new(tracks);
        for b in dom::qsa_in(&view, "[data-retry],[data-dismiss],[data-undo],[data-check],[data-replace]") {
            let (view2, ctx2, b2, tracks) = (view.clone(), ctx.clone(), b.clone(), tracks.clone());
            dom::on(&b, "click", move |_| {
                let (view, ctx, b) = (view2.clone(), ctx2.clone(), b2.clone());
                if let Some(i) = dom::data_opt(&b, "replace") {
                    let Some(t) = tracks.get(i.parse::<usize>().unwrap_or(usize::MAX)).cloned() else { return };
                    let codes: Vec<String> = t.a("issues").iter().map(|x| x.s("code")).collect();
                    let (v, c) = (view.clone(), ctx.clone());
                    report_modal("track", t.g("id").clone(), &t.s("title"), reason_for(&codes), Some(Rc::new(move |_| rerender(v.clone(), c.clone()))));
                    return;
                }
                dom::set_disabled(&b, true);
                let tracks = tracks.clone();
                dom::spawn(async move {
                    let r: Result<(), api::ApiError> = async {
                        if let Some(id) = dom::data_opt(&b, "retry") { api::post_empty(&format!("/reports/{id}/retry")).await?; toast("Looking for a replacement", "ok"); }
                        else if let Some(id) = dom::data_opt(&b, "dismiss") { api::post_empty(&format!("/reports/{id}/dismiss")).await?; toast("Dismissed", "ok"); }
                        else if let Some(id) = dom::data_opt(&b, "undo") { api::del(&format!("/blocks/{id}")).await?; toast("Unblocked – that source may be used again", "ok"); }
                        else if let Some(i) = dom::data_opt(&b, "check") {
                            let t = tracks.get(i.parse::<usize>().unwrap_or(usize::MAX)).cloned().unwrap_or(Value::Null);
                            let q = api::post_empty(&format!("/quality/tracks/{}/check", t.i("id"))).await?;
                            let bad = q.s("status") == "suspect";
                            toast(&if bad { q.s("summary") } else { "Looks fine now".to_string() }, if bad { "err" } else { "ok" });
                        }
                        Ok(())
                    }.await;
                    match r {
                        Ok(()) => rerender(view, ctx),
                        Err(e) => { toast(&e.message, "err"); dom::set_disabled(&b, false); }
                    }
                });
            });
        }
        Ok(())
    })
}
