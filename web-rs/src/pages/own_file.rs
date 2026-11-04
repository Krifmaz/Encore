//! Bring your own music: ⋯ → *Replace with my own file* on a song, and Library → *Upload your own files*
//! (a batch matched to the songs by their tags, ticked when the match is sure, replaced once you confirm).
//! The server keeps the song's id, so plays, likes, playlists and lyrics stay; the old file waits in the
//! Trash while Keep deleted files in Trash is on (else deleted). Admins, and accounts with the "own_files" permission (server: routes::own_file).
use crate::api;
use crate::app;
use crate::dom;
use crate::json::J;
use crate::ui::{esc, icon, modal, toast, ModalOpts};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{File, HtmlInputElement};

/// The file types the chooser offers (the server checks the bytes too).
pub const ACCEPT: &str = "audio/*,.flac,.mp3,.m4a,.mp4,.aac,.ogg,.opus,.wav,.aiff,.aif,.wma,.ape,.wv,.alac";

/// May this account (`user`, as `/api/auth/me` gives it) replace songs' files? Admins, and accounts with
/// the "own_files" permission.
pub fn allowed_for(user: &Value) -> bool {
    app::is_admin_user(user) || user.g("perms").truthy("own_files")
}

/// [`allowed_for`] the signed-in account.
pub fn allowed() -> bool {
    app::user().is_some_and(|u| allowed_for(&u))
}

/// The upload icon (the plus where the set has none).
fn up_icon() -> String {
    let u = icon("upload");
    if u.is_empty() { icon("plus") } else { u }
}

/// "FLAC · 24-bit / 96 kHz" of a file's facts (`{format, bit_depth, sample_rate, bitrate}`).
pub fn quality_text(f: &Value) -> String {
    let mut parts = vec![f.s("format").to_uppercase()];
    if f.i("bit_depth") > 0 && f.i("sample_rate") > 0 {
        parts.push(format!("{}-bit / {} kHz", f.i("bit_depth"), khz(f.i("sample_rate"))));
    } else if f.i("bitrate") > 0 {
        parts.push(format!("{} kbps", f.i("bitrate")));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// 44100 → "44.1", 96000 → "96".
fn khz(hz: i64) -> String {
    if hz % 1000 == 0 { (hz / 1000).to_string() } else { format!("{:.1}", hz as f64 / 1000.0) }
}

/// The line under a replaced song: "MP3 · 320 kbps → FLAC · 24-bit / 96 kHz", then any warnings.
pub fn result_text(v: &Value) -> String {
    let mut s = format!("{} → {}", quality_text(v.g("before")), quality_text(v.g("after")));
    for w in v.a("warnings") {
        if let Some(w) = w.as_str() { s.push_str(&format!(". {w}")); }
    }
    if v.g("quality").s("status") == "suspect" { s.push_str(&format!(". Quality check: {}", v.g("quality").s("summary"))); }
    s
}

/// "Artist – Title" of a song summary.
fn song_name(t: &Value) -> String {
    format!("{} – {}", t.s("artist"), t.s("title"))
}

/// The files of a file input.
fn files_of(input: &HtmlInputElement) -> Vec<File> {
    let Some(list) = input.files() else { return vec![] };
    (0..list.length()).filter_map(|i| list.get(i)).collect()
}

/// ⋯ → Replace with my own file on song `t`: choose a file, it is uploaded, checked and put in the song's
/// place. `on_done` runs once it is (the page reloads the song).
pub fn replace_modal(t: &Value, on_done: Option<Rc<dyn Fn()>>) {
    // the whole dialog takes a dropped file (crate::dropzone hands it to the input's change handler)
    let body = format!(r#"<div data-dropzone="own-file"><p class="small muted" style="margin-top:0">Your file takes the place of “{title}” for everyone. It is tagged as this song, and the song keeps its plays, likes, playlists and lyrics. The file it replaces is deleted, or kept in the Trash while Keep deleted files in Trash is on.</p>
      <label class="btn" id="own-pick">{up}Choose or drop a file…<input type="file" id="own-file" accept="{ACCEPT}" hidden></label>
      <div class="small mt" id="own-out"></div></div>"#, title = esc(&t.s("title")), up = up_icon());
    let m = modal(ModalOpts::new("Replace with my own file", &body).footer(r#"<button class="btn" data-close>Close</button>"#));
    let Some(input) = m.qs("#own-file").and_then(|e| e.dyn_into::<HtmlInputElement>().ok()) else { return };
    let id = t.i("id");
    let m = Rc::new(m);
    let m2 = m;
    dom::on(&input.clone().into(), "change", move |_| {
        let Some(file) = files_of(&input).into_iter().next() else { return };
        let (m, on_done, input) = (m2.clone(), on_done.clone(), input.clone());
        dom::spawn(async move {
            let (Some(out), Some(pick)) = (m.qs("#own-out"), m.qs("#own-pick")) else { return };
            pick.class_list().add_1("disabled").ok();
            dom::set_html(&out, &format!(r#"<span class="spinner"></span> Uploading and checking {}…"#, esc(&file.name())));
            let Ok(fd) = web_sys::FormData::new() else { return };
            let _ = fd.append_with_blob_and_filename("file", &file, &file.name());
            match api::post_form(&format!("/library/tracks/{id}/own-file"), fd).await {
                Ok(v) => {
                    dom::set_html(&out, &format!(r#"<div class="test-result ok">{} Replaced: {}</div>"#, icon("check"), esc(&result_text(&v))));
                    toast("Replaced with your file", "ok");
                    if let Some(f) = on_done { f(); }
                }
                Err(e) => {
                    dom::set_html(&out, &format!(r#"<div class="test-result err">{}</div>"#, esc(&e.message)));
                    pick.class_list().remove_1("disabled").ok();
                    input.set_value("");
                }
            }
        });
    });
}

/// One staged upload in the batch dialog: the server's answer and the song picked for it (None: skip it).
struct Row {
    /// `{token, name, format, tags, match, candidates}`
    staged: Value,
    /// the song it replaces, or None to leave it out
    pick: Option<i64>,
}

/// The song a staged upload starts with: its sure match, else none.
pub fn default_pick(staged: &Value) -> Option<i64> {
    staged.g("match").g("track").g("id").as_i64()
}

/// The `<select>` of one staged upload: its candidates (the sure one chosen) and "Leave out".
pub fn pick_select(i: usize, staged: &Value) -> String {
    let pick = default_pick(staged);
    let mut opts = r#"<option value="">Leave out</option>"#.to_string();
    for c in staged.a("candidates") {
        let t = c.g("track");
        let sel = if Some(t.i("id")) == pick { " selected" } else { "" };
        let album = if t.truthy("album") { format!(" ({})", t.s("album")) } else { String::new() };
        opts.push_str(&format!(r#"<option value="{}"{sel}>{}{}</option>"#, t.i("id"), esc(&song_name(t)), esc(&album)));
    }
    format!(r#"<select class="input" data-own="{i}" style="width:100%">{opts}</select>"#)
}

/// Library → Upload your own files: choose many files; each is uploaded and matched to the song it is by its
/// tags; the sure matches are chosen, the rest can be chosen from the likely songs or left out; *Replace N
/// songs* replaces them.
pub fn bulk_modal(on_done: Option<Rc<dyn Fn()>>) {
    let body = format!(r#"<div data-dropzone="own-files"><p class="small muted" style="margin-top:0">Choose your own copies of songs that are in the library. Each file is matched to its song by its tags (artist, title, album, length) and replaces that song's file for everyone, keeping its plays, likes, playlists and lyrics. Replaced files are deleted, or kept in the Trash while Keep deleted files in Trash is on.</p>
      <label class="btn" id="own-pick">{up}Choose or drop files…<input type="file" id="own-files" accept="{ACCEPT}" multiple hidden></label>
      <div class="stack mt" id="own-list" style="gap:8px"></div></div>"#, up = up_icon());
    let m = Rc::new(modal(ModalOpts::new("Upload your own files", &body).wide(true)
        .footer(r#"<button class="btn" data-close>Close</button><button class="btn primary" id="own-apply" disabled>Replace</button>"#)));
    let Some(input) = m.qs("#own-files").and_then(|e| e.dyn_into::<HtmlInputElement>().ok()) else { return };
    let rows: Rc<RefCell<Vec<Row>>> = Rc::new(RefCell::new(vec![]));
    // the Replace button counts the songs chosen
    let count = {
        let (m, rows) = (m.clone(), rows.clone());
        Rc::new(move || {
            let n = rows.borrow().iter().filter(|r| r.pick.is_some()).count();
            if let Some(b) = m.qs("#own-apply") {
                dom::set_text(&b, &if n == 1 { "Replace 1 song".to_string() } else { format!("Replace {n} songs") });
                dom::set_disabled(&b, n == 0);
            }
        })
    };
    {
        let (rows, count) = (rows.clone(), count.clone());
        if let Some(list) = m.qs("#own-list") {
            dom::on(&list.into(), "change", move |e| {
                let Some(sel) = dom::target_el(e) else { return };
                let Some(i) = sel.get_attribute("data-own").and_then(|s| s.parse::<usize>().ok()) else { return };
                if let Some(r) = rows.borrow_mut().get_mut(i) { r.pick = dom::value(&sel).parse().ok(); }
                count();
            });
        }
    }
    {
        let (m, rows, count, input2) = (m.clone(), rows.clone(), count, input.clone());
        dom::on(&input.into(), "change", move |_| {
            let files = files_of(&input2);
            input2.set_value("");
            if files.is_empty() { return; }
            let (m, rows, count) = (m.clone(), rows.clone(), count.clone());
            dom::spawn(async move {
                let Some(list) = m.qs("#own-list") else { return };
                let total = files.len();
                for (n, f) in files.into_iter().enumerate() {
                    let line = dom::create("div");
                    line.set_class_name("list-item");
                    dom::set_html(&line, &format!(r#"<span class="spinner"></span><span class="grow ellipsis">{} <span class="faint small">({} of {total})</span></span>"#, esc(&f.name()), n + 1));
                    let _ = list.append_child(&line);
                    let Ok(fd) = web_sys::FormData::new() else { return };
                    let _ = fd.append_with_blob_and_filename("file", &f, &f.name());
                    let res = api::post_form("/library/own-files", fd).await;
                    if !m.is_open() { return; }
                    match res {
                        Ok(v) => {
                            for staged in v.a("files") {
                                let i = rows.borrow().len();
                                let tags = staged.g("tags");
                                let what = format!("{} – {}", tags.s("artist"), tags.s("title"));
                                let note = if default_pick(&staged).is_some() { String::new() } else { r#"<span class="badge warn">Not sure – choose the song or leave it out</span>"#.to_string() };
                                dom::set_html(&line, &format!(r#"<div class="grow" style="min-width:200px"><div class="ellipsis" style="font-weight:600">{}</div>
                                    <div class="small faint ellipsis">{} · {}</div></div>
                                  <div style="min-width:260px;flex:1">{}{}</div>"#,
                                    esc(&staged.s("name")), esc(&what), esc(&staged.s("format").to_uppercase()), pick_select(i, &staged), note));
                                line.set_attribute("style", "flex-wrap:wrap;gap:10px").ok();
                                rows.borrow_mut().push(Row { pick: default_pick(&staged), staged });
                            }
                            for f in v.a("failed") {
                                dom::set_html(&line, &format!(r#"<span class="grow">{} <span class="small" style="color:var(--danger)">{}</span></span>"#, esc(&f.s("name")), esc(&f.s("reason"))));
                            }
                        }
                        Err(e) => dom::set_html(&line, &format!(r#"<span class="grow">{} <span class="small" style="color:var(--danger)">{}</span></span>"#, esc(&f.name()), esc(&e.message))),
                    }
                    count();
                }
            });
        });
    }
    // Replace: the chosen ones, all at once; then each line says how it went
    let Some(apply) = m.qs("#own-apply") else { return };
    let (m2, apply2) = (m, apply.clone());
    dom::on(&apply.into(), "click", move |_| {
        let items: Vec<Value> = rows.borrow().iter().filter_map(|r| r.pick.map(|id| json!({ "token": r.staged.s("token"), "track_id": id }))).collect();
        if items.is_empty() { return; }
        dom::set_disabled(&apply2, true);
        let (m, on_done) = (m2.clone(), on_done.clone());
        dom::spawn(async move {
            match api::post("/library/own-files/apply", json!({ "items": items })).await {
                Ok(v) => {
                    let (ok, bad) = (v.a("replaced"), v.a("failed"));
                    let mut html = String::new();
                    for r in &ok {
                        let warn: Vec<String> = r.a("warnings").iter().filter_map(|w| w.as_str().map(String::from)).collect();
                        html.push_str(&format!(r#"<div class="list-item">{} <span class="grow">{} <span class="faint small">now {}{}</span></span></div>"#,
                            icon("check"), esc(&song_name(r)), esc(&r.s("format").to_uppercase()), esc(&warn.iter().map(|w| format!(" · {w}")).collect::<String>())));
                    }
                    for r in &bad {
                        html.push_str(&format!(r#"<div class="list-item">{} <span class="grow small" style="color:var(--danger)">{}</span></div>"#, icon("x"), esc(&r.s("reason"))));
                    }
                    if let Some(list) = m.qs("#own-list") { dom::set_html(&list, &html); }
                    toast(&if ok.len() == 1 { "Replaced 1 song".to_string() } else { format!("Replaced {} songs", ok.len()) }, if bad.is_empty() { "ok" } else { "err" });
                    if let Some(f) = on_done { f(); }
                }
                Err(e) => toast(&e.message, "err"),
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_may_replace() {
        assert!(allowed_for(&json!({ "role": "admin" })));
        assert!(allowed_for(&json!({ "role": "owner" })));
        assert!(allowed_for(&json!({ "role": "user", "perms": { "own_files": true } })));
        assert!(!allowed_for(&json!({ "role": "user", "perms": { "own_files": false } })));
        assert!(!allowed_for(&json!({ "role": "user" })));
    }

    #[test]
    fn what_changed_in_words() {
        let v = json!({ "before": { "format": "mp3", "bitrate": 320 }, "after": { "format": "flac", "bit_depth": 24, "sample_rate": 44100 },
                        "warnings": ["Your file is 12s longer than the one it replaced – check it is the same version"] });
        assert_eq!(result_text(&v), "MP3 · 320 kbps → FLAC · 24-bit / 44.1 kHz. Your file is 12s longer than the one it replaced – check it is the same version");
        assert_eq!(quality_text(&json!({ "format": "flac", "bit_depth": 16, "sample_rate": 48000 })), "FLAC · 16-bit / 48 kHz");
    }

    #[test]
    fn a_sure_match_is_chosen_and_the_rest_left_out() {
        let sure = json!({ "match": { "track": { "id": 7 } }, "candidates": [{ "track": { "id": 7, "artist": "A", "title": "B", "album": "C" } }] });
        assert_eq!(default_pick(&sure), Some(7));
        let html = pick_select(0, &sure);
        assert!(html.contains(r#"<option value="7" selected>A – B (C)</option>"#), "{html}");
        let unsure = json!({ "match": null, "candidates": [{ "track": { "id": 7, "artist": "A", "title": "B" } }] });
        assert_eq!(default_pick(&unsure), None);
        assert!(!pick_select(0, &unsure).contains("selected"));
    }
}
