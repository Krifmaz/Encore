//! The file manager's small dialogs: a name (New folder, Rename) and the folder picker of Move (a folder over the
//! roots, "Move here").

use super::{logic, svg_folder, svg_up, Root};
use crate::api;
use crate::dom;
use crate::json::J;
use crate::ui::{self, esc, ModalOpts};
use futures::future::LocalBoxFuture;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

/// What a name dialog does with the name: Ok closes it, Err shows the server's words under the box and keeps it
/// open.
pub type Submit = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(), String>>>;

/// A dialog with one name box: "New folder" / "Make folder", "Rename" / "Rename". The name is checked as it is
/// typed (the server's rules); a rename selects the name without its extension.
pub fn name_dialog(title: &str, initial: &str, ok_label: &str, is_dir: bool, submit: Submit) {
    let body = format!(r#"<form id="nm-form" autocomplete="off">
        <input class="input" id="nm-in" value="{}" placeholder="Name" spellcheck="false" style="width:100%" aria-label="Name">
        <div class="small" id="nm-err" role="alert" style="color:var(--err);margin-top:8px;min-height:16px"></div>
      </form>"#, esc(initial));
    let m = ui::modal(ModalOpts::new(&esc(title), &body)
        .footer(&format!(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="nm-ok">{}</button>"#, esc(ok_label))));
    let (Some(input), Some(err), Some(ok)) = (m.qs("#nm-in"), m.qs("#nm-err"), m.qs("#nm-ok")) else { return };
    let initial = initial.to_string();
    let check = {
        let (input, err, ok, initial) = (input.clone(), err.clone(), ok.clone(), initial.clone());
        move || {
            let name = dom::value(&input).trim().to_string();
            let problem = logic::name_problem(&name);
            // an empty box says nothing yet; the button waits
            dom::set_text(&err, if name.is_empty() { "" } else { problem.unwrap_or("") });
            dom::set_disabled(&ok, problem.is_some() || (!initial.is_empty() && name == initial));
        }
    };
    check();
    {
        let check = check.clone();
        dom::on(&input, "input", move |_| check());
    }
    let busy = Rc::new(Cell::new(false));
    let go: Rc<dyn Fn()> = {
        let (m, input, err, ok, busy) = (m.clone(), input.clone(), err.clone(), ok.clone(), busy.clone());
        Rc::new(move || {
            let name = dom::value(&input).trim().to_string();
            if busy.get() || logic::name_problem(&name).is_some() || (!initial.is_empty() && name == initial) { return; }
            busy.set(true);
            dom::set_disabled(&ok, true);
            let (m, err, ok, busy, submit) = (m.clone(), err.clone(), ok.clone(), busy.clone(), submit.clone());
            dom::spawn(async move {
                match submit(name).await {
                    Ok(()) => m.close(),
                    Err(msg) => {
                        dom::set_text(&err, &msg);
                        dom::set_disabled(&ok, false);
                    }
                }
                busy.set(false);
            });
        })
    };
    {
        let go = go.clone();
        dom::on(&ok, "click", move |_| go());
    }
    if let Some(form) = m.qs("#nm-form") {
        dom::on(&form, "submit", move |e| { dom::prevent(e); go(); });
    }
    dom::focus(&input);
    if let Some(i) = input.dyn_ref::<web_sys::HtmlInputElement>() {
        let _ = i.set_selection_range(0, logic::stem_len(&dom::value(&input), is_dir) as u32);
    }
}

/// Move's folder picker: the writable roots as tabs, the folders of the one open, "Up one folder", and "Move
/// here". `exclude` (a root and paths in it) are the items being moved: a folder can't go into itself.
pub fn folder_picker(roots: Vec<Root>, start_root: &str, start_path: &str, title: &str, exclude: (String, Vec<String>), on_pick: Rc<dyn Fn(String, String)>) {
    let roots: Vec<Root> = roots.into_iter().filter(|r| !r.is_trash() && r.writable).collect();
    let Some(first) = roots.first().cloned() else {
        ui::toast("No folder here may be written to", "err");
        return;
    };
    let (root0, path0) = match roots.iter().find(|r| r.id == start_root) { Some(r) => (r.clone(), start_path.to_string()), None => (first, String::new()) };
    let body = format!(r#"<div class="tabs" id="fp-roots" style="margin-bottom:10px;max-width:100%">{}</div>
      <div class="small muted ellipsis" id="fp-where"></div>
      <div class="card flat" id="fp-list" style="margin-top:8px;max-height:min(340px, 50vh);overflow:auto"></div>"#,
        roots.iter().map(|r| format!(r#"<button type="button" data-r="{}">{}</button>"#, esc(&r.id), esc(&r.name))).collect::<String>());
    let m = ui::modal(ModalOpts::new(&esc(title), &body)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="fp-ok">Move here</button>"#));
    let Some(list) = m.qs("#fp-list") else { return };
    let at = Rc::new(RefCell::new((root0, path0)));
    let seq = Rc::new(Cell::new(0u32));
    let roots = Rc::new(roots);
    let exclude = Rc::new(exclude);
    let load: Rc<dyn Fn()> = {
        let (m, list, at, seq, exclude) = (m.clone(), list.clone(), at.clone(), seq.clone(), exclude.clone());
        Rc::new(move || {
            let (root, path) = at.borrow().clone();
            for b in m.qsa("#fp-roots [data-r]") { dom::toggle_class(&b, "active", dom::data(&b, "r") == root.id); }
            if let Some(w) = m.qs("#fp-where") { dom::set_text(&w, &logic::place(&root.name, &path)); }
            dom::set_html(&list, r#"<div class="list-item muted small">Loading…</div>"#);
            let n = seq.get() + 1;
            seq.set(n);
            let (m, list, seq, exclude) = (m.clone(), list.clone(), seq.clone(), exclude.clone());
            dom::spawn(async move {
                let res = api::get(&format!("/files/list?root={}&path={}&sort=name&dir=asc&limit=500", enc(&root.id), enc(&path))).await;
                if n != seq.get() || !m.is_open() { return; }
                let up = if path.is_empty() { String::new() } else {
                    format!(r#"<button type="button" class="list-item fp-row" data-up>{}<span class="muted">Up one folder</span></button>"#, svg_up())
                };
                match res {
                    Ok(d) => {
                        // links are left out: nothing is written through one
                        let dirs: Vec<String> = d.a("entries").iter().filter(|e| e.s("kind") == "dir").map(|e| e.s("name")).collect();
                        let rows: String = dirs.iter().map(|name| {
                            let full = logic::join(&path, name);
                            let blocked = root.id == exclude.0 && exclude.1.iter().any(|x| logic::at_or_under(&full, x));
                            format!(r#"<button type="button" class="list-item fp-row" data-d="{n}"{dis}>{ic}<span class="grow ellipsis">{n}</span></button>"#,
                                n = esc(name), dis = if blocked { " disabled" } else { "" }, ic = svg_folder())
                        }).collect();
                        let empty = if dirs.is_empty() { r#"<div class="list-item muted small">No folders here</div>"#.to_string() } else { String::new() };
                        dom::set_html(&list, &format!("{up}{rows}{empty}"));
                    }
                    Err(e) => dom::set_html(&list, &format!(r#"{up}<div class="list-item"><div class="error-box grow">Couldn't open the folder: {}</div></div>"#, esc(&e.message))),
                }
            });
        })
    };
    {
        let (at, roots, load) = (at.clone(), roots.clone(), load.clone());
        dom::on_sel(&m.el, "#fp-roots", "click", move |e| {
            let Some(b) = dom::closest(e, "[data-r]") else { return };
            let id = dom::data(&b, "r");
            let Some(r) = roots.iter().find(|r| r.id == id) else { return };
            *at.borrow_mut() = (r.clone(), String::new());
            load();
        });
    }
    {
        let (at, load) = (at.clone(), load.clone());
        dom::on(&list, "click", move |e| {
            if dom::closest(e, "[data-up]").is_some() {
                let p = logic::parent_of(&at.borrow().1);
                at.borrow_mut().1 = p;
                load();
            } else if let Some(b) = dom::closest(e, "[data-d]") {
                if b.has_attribute("disabled") { return; }
                let p = logic::join(&at.borrow().1, &dom::data(&b, "d"));
                at.borrow_mut().1 = p;
                load();
            }
        });
    }
    {
        let (m2, at) = (m.clone(), at.clone());
        dom::on_sel(&m.el, "#fp-ok", "click", move |_| {
            let (root, path) = at.borrow().clone();
            m2.close();
            on_pick(root.id, path);
        });
    }
    load();
}
