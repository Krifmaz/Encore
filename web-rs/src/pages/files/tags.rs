//! Edit tags of one audio file (§4.12, §4.13 of the client shapes). The dialog is the Fix song dialog's form
//! (Library → a song's ⋯ → Edit song details…: the same fields, labels and layout), written into the file by
//! `PUT /api/files/tags`. Only the fields that changed are sent; a field emptied clears that tag. When the file is
//! in the library, the server updates its row from the file.

use crate::api;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{self, esc, toast, ModalOpts};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// The tag writer's fields, in the order the dialog shows them, with the Fix song dialog's labels.
pub const TAGS: [(&str, &str); 8] = [
    ("title", "Title"), ("artist", "Artist"), ("album", "Album"), ("albumartist", "Album artist"),
    ("tracknumber", "Track #"), ("discnumber", "Disc #"), ("date", "Year"), ("genre", "Genre"),
];

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// The fields whose value differs from the file's (as `{key: value}`; "" clears that tag).
pub fn changed(original: &Value, now: &[(String, String)]) -> Map<String, Value> {
    now.iter().filter(|(k, v)| original.s(k) != *v).map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect()
}

/// The form of the dialog: the Fix song dialog's grid of labelled boxes.
pub fn form_html(tags: &Value) -> String {
    format!(r#"<div class="form-grid">{}</div>"#, TAGS.iter().map(|(k, l)| format!(
        r#"<label class="field"><span>{l}</span><input class="input" data-tag="{k}" value="{}"{}></label>"#,
        esc(&tags.s(k)), if matches!(*k, "tracknumber" | "discnumber" | "date") { r#" inputmode="numeric""# } else { "" })).collect::<String>())
}

/// Opens the dialog for `path` in `root`; `on_saved` runs after the tags were written.
pub fn tags_modal(root: String, path: String, on_saved: Rc<dyn Fn()>) {
    let name = crate::pages::files::logic::last_part(&path).to_string();
    let m = ui::modal(ModalOpts::new("Edit tags", &format!(r#"<p class="small muted ellipsis" style="margin-top:0" title="{n}">{n}</p>
      <div id="tg-body"><div class="empty" style="padding:30px"><span class="spinner"></span></div></div>"#, n = esc(&name)))
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="tg-save" disabled>Save</button>"#)
        .wide(true));
    let original: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
    {
        let (m, root, path, original) = (m.clone(), root.clone(), path.clone(), original.clone());
        dom::spawn(async move {
            let res = api::get(&format!("/files/tags?root={}&path={}", enc(&root), enc(&path))).await;
            let Some(body) = m.qs("#tg-body") else { return };
            match res {
                Ok(d) => {
                    let tags = d.o("tags");
                    dom::set_html(&body, &form_html(&tags));
                    *original.borrow_mut() = Some(tags);
                    if let Some(b) = m.qs("#tg-save") { dom::set_disabled(&b, false); }
                    if let Some(first) = m.qs("[data-tag]") { dom::focus(&first); }
                }
                Err(e) => dom::set_html(&body, &format!(r#"<div class="error-box row" style="align-items:flex-start"><span class="grow">Couldn't read the tags: {}</span>{}</div>"#,
                    esc(&e.message), crate::details::copy_button(&format!("Couldn't read the tags: {}", e.message)))),
            }
        });
    }
    let m2 = m.clone();
    dom::on_sel(&m.el, "#tg-save", "click", move |_| {
        let (m, root, path, on_saved) = (m2.clone(), root.clone(), path.clone(), on_saved.clone());
        let Some(orig) = original.borrow().clone() else { return };
        let now: Vec<(String, String)> = m.qsa("[data-tag]").iter().map(|i| (dom::data(i, "tag"), dom::value(i))).collect();
        let diff = changed(&orig, &now);
        if diff.is_empty() { m.close(); return; }
        dom::spawn(async move {
            let btn = m.qs("#tg-save");
            if let Some(b) = &btn { dom::set_disabled(b, true); dom::set_text(b, "Saving…"); }
            match api::put("/files/tags", json!({ "root": root, "path": path, "tags": Value::Object(diff) })).await {
                Ok(_) => {
                    toast("Tags saved", "ok");
                    m.close();
                    on_saved();
                }
                Err(e) => {
                    toast(if e.message.is_empty() { "Couldn't write the tags" } else { &e.message }, "err");
                    if let Some(b) = &btn { dom::set_disabled(b, false); dom::set_text(b, "Save"); }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_fields_are_sent_and_empty_clears() {
        let orig = json!({"title": "crazy", "artist": "Gnarls Barkley", "album": "", "genre": "Soul"});
        let now = vec![("title".to_string(), "Crazy".to_string()), ("artist".to_string(), "Gnarls Barkley".to_string()),
            ("album".to_string(), "".to_string()), ("genre".to_string(), "".to_string())];
        let d = changed(&orig, &now);
        assert_eq!(Value::Object(d), json!({"title": "Crazy", "genre": ""}));
        let html = form_html(&json!({"title": "A \"quoted\" <title>"}));
        assert!(html.contains(r#"data-tag="title" value="A &quot;quoted&quot; &lt;title&gt;""#), "{html}");
        assert_eq!(html.matches("data-tag=").count(), 8);
        assert!(html.contains("<span>Album artist</span>") && html.contains("<span>Track #</span>"));
    }
}
