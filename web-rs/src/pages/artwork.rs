//! Port of web/js/pages/artwork.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Pick an album's artwork by hand: search every catalogue or upload a picture; it's embedded in the files.

use crate::api;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{esc, esck, modal, toast, ModalOpts};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// `artworkModal(albumId, a, onDone)`.
pub fn artwork_modal(album_id: &str, a: &Value, on_done: Rc<dyn Fn()>) {
    let album_id = album_id.to_string();
    let m = modal(ModalOpts::new("Change artwork", &format!(r#"<div data-dropzone="aw-file"><form class="row wrap" id="aw-form"><input class="input grow" name="q" value="{}" placeholder="Album title to search for" style="min-width:0">
        <button class="btn primary">Search</button>
        <label class="btn">Upload…<input type="file" accept="image/jpeg,image/png" id="aw-file" hidden></label></form>
      <div class="small muted mt">Cover Art Archive, Deezer, Apple Music, YouTube Music and Discogs. The picture is embedded in every song and saved as cover.jpg.</div>
      <div id="aw-out" class="mt"></div></div>"#, esck(a, "album"))).wide(true));
    let out = m.qs("#aw-out").expect("#aw-out");

    // apply(body, el): embed the chosen picture, then refresh every cover of this album on the page
    let apply: Rc<dyn Fn(Value, Option<Element>)> = {
        let album_id = album_id.clone();
        let m = m.clone();
        Rc::new(move |body: Value, el: Option<Element>| {
            if let Some(el) = &el { dom::add_class(el, "busy"); }
            let album_id = album_id.clone();
            let m = m.clone();
            let on_done = on_done.clone();
            dom::spawn(async move {
                match api::post(&format!("/library/albums/{album_id}/art"), body).await {
                    Ok(r) => {
                        toast(&format!("Artwork embedded in {} file(s)", r.s("embedded")), "ok");
                        m.close();
                        // show the new picture everywhere on the page right away
                        let v = dom::now_ms() as i64;
                        let re = js_sys::RegExp::new(r"[?&]v=\d+", "");
                        for img in dom::qsa(&format!(r#"img[src*="/albums/{album_id}/cover"]"#)) {
                            let Ok(img) = img.dyn_into::<web_sys::HtmlImageElement>() else { continue };
                            let src = img.src();
                            let stripped: String = js_sys::JsString::from(src.as_str()).replace_by_pattern(&re, "").into();
                            img.set_src(&format!("{stripped}{}v={v}", if src.contains('?') { "&" } else { "?" }));
                        }
                        on_done();
                    }
                    Err(e) => {
                        toast(&e.message, "err");
                        if let Some(el) = &el { dom::remove_class(el, "busy"); }
                    }
                }
            });
        })
    };

    // search(q): the candidates grid; `out.onclick` is replaced on every search like the JS did
    let click: Rc<RefCell<Option<dom::EventListener>>> = Rc::new(RefCell::new(None));
    let search: Rc<dyn Fn(String)> = {
        let apply = apply.clone();
        Rc::new(move |q: String| {
            dom::set_html(&out, r#"<div class="muted"><span class="spinner"></span> Searching every catalogue…</div>"#);
            let album_id = album_id.clone();
            let out = out.clone();
            let apply = apply.clone();
            let click = click.clone();
            dom::spawn(async move {
                let q = String::from(js_sys::encode_uri_component(&q));
                match api::get(&format!("/library/albums/{album_id}/art-candidates?q={q}")).await {
                    Ok(c) => {
                        let c = c.items();
                        let html = if !c.is_empty() {
                            format!(r#"<div class="grid art-grid">{}</div>"#, c.iter().enumerate().map(|(i, x)| format!(r#"<button class="tile art-pick" data-i="{i}" title="{}">
          <div class="art"><img src="{}" alt="" loading="lazy"></div>
          <div class="s ellipsis">{}{}</div><div class="s ellipsis faint">{}</div></button>"#,
                                esck(x, "label"), esck(x, "thumb"), esck(x, "source"),
                                if x.truthy("size") { format!(" · {}", esc(&x.s("size"))) } else { String::new() },
                                esck(x, "label"))).collect::<String>())
                        } else {
                            r#"<div class="muted">No artwork found – try another title, or upload a picture.</div>"#.to_string()
                        };
                        dom::set_html(&out, &html);
                        let apply = apply.clone();
                        *click.borrow_mut() = Some(dom::listener(&out, "click", move |e| {
                            let Some(b) = dom::closest(e, "[data-i]") else { return };
                            let Ok(i) = dom::data(&b, "i").parse::<usize>() else { return };
                            let Some(x) = c.get(i) else { return };
                            apply(json!({ "url": x.g("url").clone() }), Some(b));
                        }));
                    }
                    Err(e) => dom::set_html(&out, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))),
                }
            });
        })
    };

    {
        let search = search.clone();
        dom::on_sel(&m.el, "#aw-form", "submit", move |e| {
            dom::prevent(e);
            let q = dom::target_el(e)
                .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
                .and_then(|f| web_sys::FormData::new_with_form(&f).ok())
                .map(|fd| fd.get("q").as_string().unwrap_or_default())
                .unwrap_or_default();
            search(q);
        });
    }
    {
        let apply = apply.clone();
        dom::on_sel(&m.el, "#aw-file", "change", move |e| {
            let Some(f) = dom::target_el(e)
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                .and_then(|i| i.files())
                .and_then(|fl| fl.get(0)) else { return };
            if f.size() > 15.0 * 1024.0 * 1024.0 { return toast("Picture is larger than 15 MB", "err"); }
            let Ok(rd) = web_sys::FileReader::new() else { return };
            let apply = apply.clone();
            let rd2 = rd.clone();
            dom::keep(dom::EventListener::once(&rd, "load", move |_| {
                let data = rd2.result().ok().and_then(|v| v.as_string()).unwrap_or_default();
                apply(json!({ "data": data }), None);
            }));
            let _ = rd.read_as_data_url(&f);
        });
    }
    search(a.s("album"));
}
