//! The Apple Music wrapper's block on its Plugins card (under its fields): the wrapper's state, the Apple
//! Music app upload, Set up / Start / Stop and the container's log for the wrapper Encore runs, the Apple ID
//! sign-in with the two-factor code Apple sends, and "Use with gamdl". It draws what GET
//! /api/system/wrapper answers ([`panel_html`], a pure function the tests check) and asks again every three
//! seconds while a setup runs.
//!
//! The password goes in the sign-in request only: it is never kept in the page, the settings or a log.
use crate::api;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{esc, fmt, icon, icon_attr, toast};
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, FormData, HtmlInputElement};

/// The spinner of a step in progress.
const SPINNER: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;

/// The block's frame; [`bind`] fills it.
pub fn block() -> String {
    r##"<div class="card flat mt" id="wrapper-panel" style="padding:12px 14px">
    <div id="wrapper-body" class="small muted">Checking the wrapper…</div>
    <div class="small mt" id="wrapper-out" hidden></div>
    <pre class="mt small" id="wrapper-log" hidden style="max-height:320px;overflow:auto;white-space:pre-wrap"></pre>
  </div>"##.to_string()
}

/// One button of the block (`data-wr` names its action).
fn button(act: &str, icon_name: &str, label: &str, primary: bool) -> String {
    format!(r##"<button class="btn sm{}" type="button" data-wr="{act}">{}{}</button>"##, if primary { " primary" } else { "" }, icon(icon_name), esc(label))
}

/// A line with a coloured badge.
fn badge(kind: &str, text: &str) -> String {
    format!(r##"<span class="badge {kind}">{}</span>"##, esc(text))
}

/// The managed wrapper's part: the Docker socket, the app, the image, the container, the setup job.
fn managed_html(o: &Value) -> String {
    let docker = o.g("docker");
    if !docker.truthy("ok") {
        return format!(r##"<div class="row wrap" style="gap:8px;align-items:center">{}<span>{}</span></div>
    <div class="small muted mt">Encore runs the wrapper as a container next to its own. Add <code>- /var/run/docker.sock:/var/run/docker.sock</code> to Encore's volumes and restart it, or choose "A wrapper I run" above.</div>"##,
            badge("warn", "No Docker"), esc(&docker.s("reason")));
    }
    let job = o.g("job");
    let running = job.truthy("running");
    let built = o.truthy("image_built");
    let builds = o.truthy("builds");
    // no container in the answer reads as none made yet
    let container = if o.truthy("container") { o.s("container") } else { "missing".to_string() };
    let bundle = o.g("bundle");
    let mut html = String::new();
    // the app (only when Encore builds the image)
    if builds {
        let app = if bundle.is_object() {
            // the icon sized inline: a bare svg in this div has no size of its own and fills the card
            format!("{} {} · {} · {}", icon_attr("check", r#"width="14" height="14" style="vertical-align:-2px""#),
                esc(&bundle.s("name")), esc(&bundle.s("kind")), fmt::size(bundle.f("size")))
        } else {
            "No app uploaded yet: Apple Music for Android 3.6.0-beta (build 1109), as an .apk or an APKMirror .apkm bundle.".to_string()
        };
        html.push_str(&format!(r##"<div class="row between wrap" style="gap:8px;align-items:center" data-dropzone>
      <div style="min-width:0"><b>Apple Music app</b><div class="small muted" style="overflow-wrap:anywhere">{app}</div></div>
      <label class="btn sm" style="cursor:pointer">{up}{label}<input type="file" id="wrapper-app" accept=".apk,.apkm,application/vnd.android.package-archive,application/zip" hidden></label>
    </div>"##, up = icon("plus"), label = if bundle.is_object() { "Replace app" } else { "Upload app" }));
    }
    // the container
    let state = match container.as_str() {
        "running" => badge("ok", "Running"),
        "missing" if built => badge("warn", "Built, not started"),
        "missing" => badge("warn", "Not set up"),
        "exited" | "created" => badge("warn", "Stopped"),
        other => badge("warn", other),
    };
    let mut buttons = Vec::new();
    if !running {
        if !built || container == "missing" {
            buttons.push(button("set-up", "download", if built { "Start" } else { "Set up" }, true));
        } else if container != "running" {
            buttons.push(button("start", "play", "Start", true));
        }
        if container == "running" {
            buttons.push(button("stop", "pause", "Stop", false));
        }
        if built && builds {
            buttons.push(button("rebuild", "refresh", "Build again", false));
        }
    }
    if container != "missing" {
        buttons.push(button("log", "list", "Wrapper log", false));
    }
    html.push_str(&format!(r##"<div class="row between wrap mt" style="gap:8px;align-items:center">
      <div style="min-width:0"><b>Wrapper</b> {state}<div class="small muted">{image}</div></div>
      <div class="row wrap" style="gap:8px">{buttons}</div>
    </div>"##, image = esc(&o.s("image")), buttons = buttons.join("")));
    if running {
        html.push_str(&format!(r##"<div class="small mt">{SPINNER} {}…</div>"##, esc(&job.s("step"))));
    } else if job.truthy("error") {
        html.push_str(&format!(r##"<div class="test-result err mt">{}<span>{}</span></div>"##, icon("x"), esc(&job.s("error"))));
    }
    html
}

/// The account's part: sign in, the code, or who is signed in.
fn account_html(o: &Value) -> String {
    let a = o.g("account");
    if !a.is_object() {
        return String::new();
    }
    let state = a.s("state");
    let body = match state.as_str() {
        "authenticated" => {
            let who = if a.truthy("apple_id") { format!("Signed in as {}", a.s("apple_id")) } else { "Signed in".to_string() };
            let sf = if a.truthy("storefront") { format!(" · storefront {}", a.s("storefront")) } else { String::new() };
            format!(r##"<div class="row between wrap" style="gap:8px;align-items:center"><div>{} {}</div>{}</div>"##,
                badge("ok", "Apple Music"), esc(&format!("{who}{sf}")), button("sign-out", "x", "Sign out", false))
        }
        "awaiting_2fa" => format!(r##"<div>Apple sent a six-digit code to your iPhone, iPad or Mac{}.</div>
      <div class="row wrap mt" style="gap:8px"><input id="wrapper-code" inputmode="numeric" autocomplete="one-time-code" maxlength="9" placeholder="123456" style="width:9em">{}{}</div>"##,
            if a.truthy("apple_id") { format!(" for {}", esc(&a.s("apple_id"))) } else { String::new() },
            button("code", "check", "Send code", true), button("sign-out", "x", "Cancel", false)),
        "unreachable" => format!(r##"<div class="test-result err">{}<span>{}</span></div>"##, icon("x"), esc(&a.s("error"))),
        _ => {
            let failed = if state == "failed" && a.truthy("error") { format!(r##"<div class="test-result err">{}<span>{}</span></div>"##, icon("x"), esc(&a.s("error"))) } else { String::new() };
            format!(r##"{failed}<div class="small muted">Sign the wrapper in with the Apple ID of your Apple Music subscription. The password goes to the wrapper only; Encore keeps neither.</div>
      <div class="row wrap mt" style="gap:8px"><input id="wrapper-apple-id" type="email" autocomplete="username" placeholder="Apple ID" style="min-width:14em"><input id="wrapper-password" type="password" autocomplete="current-password" placeholder="Password" style="min-width:12em">{}</div>"##,
                button("sign-in", "link", "Sign in", true))
        }
    };
    format!(r##"<div class="mt"><b>Apple ID</b><div class="mt">{body}</div></div>"##)
}

/// The gamdl part: whether gamdl's Wrapper URL is this wrapper's.
fn gamdl_html(o: &Value) -> String {
    let g = o.g("gamdl");
    if !o.truthy("url") {
        return String::new();
    }
    let line = if g.truthy("points_here") {
        format!("{} gamdl's Wrapper URL is this wrapper ({})", badge("ok", "gamdl"), esc(&g.s("url")))
    } else if g.truthy("locked") {
        format!("{} GAMDL_WRAPPER_URL in Encore's environment sets gamdl's Wrapper URL ({})", badge("warn", "gamdl"), esc(&g.s("url")))
    } else {
        let now = if g.truthy("url") { format!("it uses {}", esc(&g.s("url"))) } else { "it has no Wrapper URL".to_string() };
        format!(r##"{} gamdl does not use this wrapper yet: {now} {}"##, badge("warn", "gamdl"), button("gamdl", "link", "Use with gamdl", false))
    };
    format!(r##"<div class="row wrap mt" style="gap:8px;align-items:center">{line}</div>"##)
}

/// Shows on the gamdl card, as it stands on the page, the Wrapper URL and decrypt address this wrapper just
/// filled in (`g`: the overview's `gamdl`): their inputs take the values, read-only, with the plugin's note
/// under them, as a reload of the page would draw them. Nothing changes while gamdl points elsewhere.
fn sync_gamdl_fields(g: &Value) {
    if !g.truthy("points_here") { return; }
    for (key, v) in [("gamdl_wrapper_url", g.s("url")), ("gamdl_wrapper_decrypt", g.s("decrypt"))] {
        for input in dom::qsa(&format!(r#"input[data-k="{key}"]"#)) {
            if input.has_attribute("disabled") { continue; } // the environment's value stays
            dom::set_value(&input, &v);
            let _ = input.set_attribute("readonly", "");
            let Some(field) = dom::closest_of(&input, ".field") else { continue };
            if dom::qs_in(&field, "small.src.plugin").is_some() { continue; }
            // the plugin's note takes the place of "Default · env var …"
            for old in dom::qsa_in(&field, "small.src") { dom::remove(&old); }
            let note = dom::create("small");
            let _ = note.set_attribute("class", "src plugin");
            dom::set_html(&note, "🔗 Filled in by the <b>Apple Music wrapper</b> plugin – it changes this itself");
            let _ = field.append_child(&note);
        }
    }
}

/// The block's body for the answer of GET /api/system/wrapper.
pub fn panel_html(o: &Value) -> String {
    let mut html = String::new();
    if o.s("mode") == "external" {
        html.push_str(&if o.truthy("url") {
            format!(r##"<div><b>Wrapper</b> <span class="muted">{}</span></div>"##, esc(&o.s("url")))
        } else {
            r##"<div class="muted">Enter your wrapper-v2's address above and save the card.</div>"##.to_string()
        });
    } else {
        html.push_str(&managed_html(o));
    }
    html.push_str(&account_html(o));
    html.push_str(&gamdl_html(o));
    html
}

/// Fill the block of `card` and keep it current while `view` is shown.
pub fn bind(view: &Element, card: &Element) {
    let (Some(body), Some(out), Some(log)) = (dom::qs_in(card, "#wrapper-body"), dom::qs_in(card, "#wrapper-out"), dom::qs_in(card, "#wrapper-log")) else { return };
    // ask the server and paint; true while a setup runs
    let busy = Rc::new(Cell::new(false));
    let refresh: Rc<dyn Fn()> = {
        let (body, busy) = (body.clone(), busy.clone());
        Rc::new(move || {
            let (body, busy) = (body.clone(), busy.clone());
            dom::spawn(async move {
                match api::get("/system/wrapper").await {
                    Ok(o) => {
                        busy.set(o.g("job").truthy("running"));
                        dom::set_html(&body, &panel_html(&o));
                        sync_gamdl_fields(o.g("gamdl"));
                    }
                    Err(e) => dom::set_text(&body, &e.message),
                }
            });
        })
    };
    refresh();
    // the answer of an action, under the block
    let say: Rc<dyn Fn(bool, &str)> = {
        let out = out.clone();
        Rc::new(move |ok: bool, msg: &str| {
            dom::hide(&out, false);
            dom::set_html(&out, &super::test_result(&json!({ "ok": ok, "message": msg })));
        })
    };
    {
        let (card2, refresh, say, log) = (card.clone(), refresh.clone(), say.clone(), log.clone());
        dom::on(card, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-wr]") else { return };
            dom::prevent(e);
            if b.has_attribute("disabled") { return; }
            let act = dom::data(&b, "wr");
            let (card, refresh, say, log) = (card2.clone(), refresh.clone(), say.clone(), log.clone());
            dom::set_disabled(&b, true);
            dom::spawn(async move {
                let r = match act.as_str() {
                    "set-up" => api::post("/system/wrapper/set-up", json!({})).await.map(|_| json!({ "ok": true, "message": "Setting up: this runs in the background" })),
                    "rebuild" => api::post("/system/wrapper/set-up", json!({ "rebuild": true })).await.map(|_| json!({ "ok": true, "message": "Building again: this runs in the background" })),
                    "start" => api::post_empty("/system/wrapper/start").await,
                    "stop" => api::post_empty("/system/wrapper/stop").await,
                    "gamdl" => api::post_empty("/system/wrapper/gamdl").await,
                    "sign-out" => api::del("/system/wrapper/sign-in").await,
                    "sign-in" => {
                        let (id, pw) = (dom::value_of(&card, "#wrapper-apple-id"), dom::value_of(&card, "#wrapper-password"));
                        // the password leaves the field at once, whatever the answer
                        if let Some(p) = dom::qs_in(&card, "#wrapper-password") { dom::set_value(&p, ""); }
                        say(true, "Asking Apple…");
                        api::post("/system/wrapper/sign-in", json!({ "apple_id": id, "password": pw })).await
                    }
                    "code" => {
                        let code = dom::value_of(&card, "#wrapper-code");
                        api::post("/system/wrapper/sign-in/code", json!({ "code": code })).await
                    }
                    "log" => {
                        let shown = !log.has_attribute("hidden");
                        if shown { dom::hide(&log, true); } else {
                            match api::get("/system/wrapper/logs?lines=300").await {
                                Ok(v) => { dom::set_text(&log, &if v.s("text").is_empty() { "(nothing yet)".to_string() } else { v.s("text") }); dom::hide(&log, false); }
                                Err(e) => toast(&e.message, "err"),
                            }
                        }
                        dom::set_disabled(&b, false);
                        return;
                    }
                    _ => return,
                };
                match r {
                    Ok(v) => {
                        // a sign-in step answers with the account's state: a refusal is not ok
                        let ok = !(v.s("state") == "failed");
                        let msg = if v.truthy("message") { v.s("message") } else { "Done".to_string() };
                        say(ok, &msg);
                    }
                    Err(e) => say(false, &e.message),
                }
                dom::set_disabled(&b, false);
                refresh();
            });
        });
    }
    {
        let (refresh, say) = (refresh.clone(), say.clone());
        dom::on(card, "change", move |e| {
            let Some(t) = dom::target_el(e) else { return };
            if t.id() != "wrapper-app" { return; }
            let Some(input) = t.dyn_ref::<HtmlInputElement>() else { return };
            let Some(f) = input.files().and_then(|l| l.get(0)) else { return };
            input.set_value("");
            let (refresh, say) = (refresh.clone(), say.clone());
            dom::spawn(async move {
                say(true, "Uploading the app…");
                let Ok(fd) = FormData::new() else { return };
                let _ = fd.append_with_blob_and_filename("file", &f, &f.name());
                match api::post_form("/system/wrapper/app", fd).await {
                    Ok(v) => say(true, &format!("Kept {} ({}): press Set up", v.g("bundle").s("name"), v.g("bundle").s("kind"))),
                    Err(e) => say(false, &e.message),
                }
                refresh();
            });
        });
    }
    // every three seconds while a setup runs and the card is open
    let card = card.clone();
    dom::interval(view, 3000, move || {
        if busy.get() && dom::is_connected(&card) && card.has_attribute("open") { refresh(); }
    });
}

#[cfg(test)]
mod tests {
    use super::panel_html;
    use serde_json::json;

    #[test]
    fn no_docker_says_how_to_mount_the_socket() {
        let h = panel_html(&json!({ "mode": "managed", "docker": { "ok": false, "reason": "The Docker socket is not mounted" }, "gamdl": {} }));
        assert!(h.contains("No Docker") && h.contains("/var/run/docker.sock:/var/run/docker.sock"), "{h}");
        assert!(!h.contains("data-wr=\"set-up\""), "nothing to set up without Docker");
    }

    #[test]
    fn the_steps_follow_the_wrapper() {
        let base = || json!({ "mode": "managed", "docker": { "ok": true }, "builds": true, "image": "encore-wrapper-v2:100e0a864e88", "gamdl": {}, "job": {} });
        // nothing uploaded: Upload app and Set up
        let h = panel_html(&base());
        assert!(h.contains("Upload app") && h.contains("data-wr=\"set-up\"") && h.contains("Not set up"), "{h}");
        // a setup running: its step, no buttons to start another
        let mut o = base();
        o["job"] = json!({ "running": true, "step": "Building the image: Step 7/24 : RUN cargo build" });
        let h = panel_html(&o);
        assert!(h.contains("Step 7/24") && !h.contains("data-wr=\"set-up\""), "{h}");
        // running, waiting for the code
        let mut o = base();
        o["image_built"] = json!(true);
        o["container"] = json!("running");
        o["url"] = json!("http://encore-wrapper");
        o["bundle"] = json!({ "name": "apple-music.apkm", "kind": "APKM bundle", "size": 157286400 });
        o["account"] = json!({ "state": "awaiting_2fa", "apple_id": "me@example.com" });
        let h = panel_html(&o);
        assert!(h.contains("Running") && h.contains("Replace app") && h.contains("150.0 MB"), "{h}");
        assert!(h.contains("id=\"wrapper-code\"") && h.contains("for me@example.com") && h.contains("data-wr=\"stop\""), "{h}");
        assert!(h.contains("Use with gamdl"), "gamdl does not point here yet: {h}");
        // signed in, gamdl pointed here
        o["account"] = json!({ "state": "authenticated", "apple_id": "me@example.com", "storefront": "us" });
        o["gamdl"] = json!({ "url": "http://encore-wrapper", "points_here": true });
        let h = panel_html(&o);
        assert!(h.contains("Signed in as me@example.com · storefront us") && h.contains("data-wr=\"sign-out\""), "{h}");
        assert!(h.contains("gamdl&#39;s Wrapper URL is this wrapper") || h.contains("gamdl's Wrapper URL is this wrapper"), "{h}");
        assert!(!h.contains("Use with gamdl"));
    }

    #[test]
    fn an_external_wrapper_has_no_docker_steps() {
        let h = panel_html(&json!({ "mode": "external", "url": "http://wrapper-v2", "gamdl": {}, "account": { "state": "logged_out" } }));
        assert!(h.contains("http://wrapper-v2") && h.contains("id=\"wrapper-password\"") && !h.contains("data-wr=\"set-up\""), "{h}");
        let h = panel_html(&json!({ "mode": "external", "gamdl": {} }));
        assert!(h.contains("Enter your wrapper-v2"), "{h}");
    }
}
