//! Port of web/js/pages/diagnostics.js – see docs/archive/ui-rewrite/CONTRACT.md
use crate::api::{self, ApiError};
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, truthy, J};
use crate::ui::{confirm_dialog, copy_text, esc, esck, fmt, icon, toast};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, FormData, HtmlInputElement};

const KIND: [(&str, &str); 3] = [("service", "Connections"), ("path", "Folders & permissions"), ("tool", "Tools in the container")];

/// `${obj[key]}` / `String(obj[key])` as JavaScript prints it (null → "null", missing → "undefined").
fn js_str(v: &Value, key: &str) -> String {
    if !v.has(key) { return "undefined".into(); }
    match v.g(key) { Value::Null => "null".into(), x => js_string(x) }
}
/// `String(obj[key] || '')`.
fn or_empty(v: &Value, key: &str) -> String { if v.truthy(key) { v.s(key) } else { String::new() } }

/// The VPN for Soulseek line (Settings → Soulseek and the Diagnostics row; the same words in web/js):
/// "Connected · port 51234 · public IP 1.2.3.4 · last handshake 12 s ago" / "Connecting…" / the error / "Off".
pub fn vpn_text(v: &Value) -> String {
    if !v.is_object() { return "Off".into(); }
    match v.g("state").as_str() {
        Some("up") => {
            let mut parts = vec!["Connected".to_string()];
            if !v.is_nullish("forwarded_port") { parts.push(format!("port {}", v.s("forwarded_port"))); }
            else if v.truthy("port_forwarding") {
                parts.push(if v.truthy("natpmp_error") { format!("no forwarded port ({})", v.s("natpmp_error")) } else { "no forwarded port yet".to_string() });
            }
            if v.truthy("public_ip") { parts.push(format!("public IP {}", v.s("public_ip"))); }
            if !v.is_nullish("last_handshake_secs_ago") {
                let n = crate::playback::js_round(v.f("last_handshake_secs_ago"));
                parts.push(format!("last handshake {} s ago", js_string(&json!(n))));
            }
            parts.join(" · ")
        }
        Some("connecting") => "Connecting…".into(),
        Some("error") => if v.truthy("error") { v.s("error") } else { "The VPN could not connect".into() },
        _ => "Off".into(),
    }
}

/// `vpn_text` as HTML: errors in red.
pub fn vpn_state_html(v: &Value) -> String {
    let t = esc(&vpn_text(v));
    if v.g("state").as_str() == Some("error") { format!(r#"<span style="color:var(--err)">{t}</span>"#) } else { t }
}

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Diagnostics</h1><div class="small muted">Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.</div></div>
      <div class="row"><button class="btn" id="copy" disabled>{copy}Copy report</button><button class="btn primary" id="run">{refresh}Run checks</button></div>
    </div>
    <div id="diag"><div class="empty"><span class="spinner"></span></div></div>
    <div id="updates"></div>
    <div class="section-head" data-dropzone="bk-upload"><h2>Backups</h2>
      <div class="row"><label class="btn sm">{up}Upload a backup…<input type="file" id="bk-upload" accept=".db,.sqlite,.sqlite3,application/vnd.sqlite3,application/x-sqlite3" hidden></label><button class="btn sm primary" id="bk-now">Back up now</button></div>
    </div>
    <div class="card flat" data-dropzone="bk-upload">
      <div class="small muted" style="margin-bottom:10px">Encore keeps copies of its database here (daily, and before every upgrade). Download one now and then to keep it somewhere else.</div>
      <div id="backups"><div class="empty"><span class="spinner"></span></div></div>
    </div>
    <div class="section-head"><h2>App crashes</h2><button class="btn sm" id="cr-clear" disabled>Clear</button></div>
    <div class="card flat">
      <div class="small muted" style="margin-bottom:10px">The phone app sends a report the next time it opens after it crashed or the system closed it. The newest 50 are kept.</div>
      <div id="crashes"><div class="empty"><span class="spinner"></span></div></div>
    </div>"#, copy = icon("copy"), refresh = icon("refresh"),
            // `icon.upload || icon.plus`
            up = { let u = icon("upload"); if u.is_empty() { icon("plus") } else { u } }));
        let Some(diag) = dom::qs_in(&view, "#diag") else { return Err("#diag is missing".into()) };
        let Some(bk) = dom::qs_in(&view, "#backups") else { return Err("#backups is missing".into()) };
        {
            let bk2 = bk.clone();
            dom::on_sel(&view, "#bk-now", "click", move |e| {
                let Some(btn) = dom::target_el(e) else { return };
                let bk = bk2.clone();
                dom::spawn(async move {
                    dom::set_disabled(&btn, true);
                    match api::post_empty("/system/backups").await {
                        Ok(d) => {
                            toast(&format!("Backed up to backups/{}", d.g("created").s("name")), "ok");
                            draw_backups(&bk, &d);
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&btn, false);
                });
            });
        }
        {
            let bk2 = bk.clone();
            dom::on_sel(&view, "#bk-upload", "change", move |e| {
                let Some(input) = dom::target_el(e).and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else { return };
                let Some(f) = input.files().and_then(|l| l.get(0)) else { return };
                input.set_value("");
                let bk = bk2.clone();
                dom::spawn(async move {
                    dom::set_html(&bk, r#"<div class="empty"><span class="spinner"></span><p class="muted">Uploading and checking…</p></div>"#);
                    let Ok(fd) = FormData::new() else { return };
                    let _ = fd.append_with_blob("file", &f);
                    let req = gloo_net::http::RequestBuilder::new("/api/system/backups/upload")
                        .method(gloo_net::http::Method::POST)
                        .header("X-Requested-With", "encore")
                        .credentials(web_sys::RequestCredentials::SameOrigin)
                        .body(fd);
                    let Ok(req) = req else { return };
                    let Ok(r) = req.send().await else { return };
                    if !r.ok() {
                        // (await r.json().catch(() => ({}))).detail || 'Upload failed'
                        let data: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
                        let detail = data.g("detail");
                        toast(&if truthy(detail) { js_string(detail) } else { "Upload failed".to_string() }, "err");
                        load_backups(&bk).await;
                        return;
                    }
                    let d: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
                    toast(&format!("Saved as backups/{}", d.g("created").s("name")), "ok");
                    draw_backups(&bk, &d);
                });
            });
        }
        {
            let bk = bk.clone();
            dom::spawn(async move { load_backups(&bk).await });
        }
        // Updates (admins only; the server answers 403 to anyone else and an older server 404: then nothing shows)
        if let Some(up) = dom::qs_in(&view, "#updates") {
            if crate::app::is_admin() { dom::spawn(async move { load_updates(&up, false).await }); }
        }
        if let Some(cr) = dom::qs_in(&view, "#crashes") {
            let v2 = view.clone();
            let cr2 = cr.clone();
            dom::on_sel(&view, "#cr-clear", "click", move |_| {
                let (v, cr) = (v2.clone(), cr2.clone());
                dom::spawn(async move {
                    if !confirm_dialog("Clear app crashes?", "All crash reports from the phone app are removed.", "Clear", true).await { return; }
                    if let Err(e) = api::del("/diagnostics/client-crash").await { toast(&e.message, "err"); }
                    load_crashes(&v, &cr).await;
                });
            });
            let v = view.clone();
            dom::spawn(async move { load_crashes(&v, &cr).await });
        }
        let last: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        {
            let (v, d, l) = (view.clone(), diag.clone(), last.clone());
            dom::on_sel(&view, "#run", "click", move |_| {
                let (v, d, l) = (v.clone(), d.clone(), l.clone());
                // the JS `onclick = run`: a failure here is an unhandled rejection (console only)
                dom::spawn(async move { if let Err(e) = run(v, d, l).await { dom::console_error(&e.message); } });
            });
        }
        {
            let l = last.clone();
            dom::on_sel(&view, "#copy", "click", move |_| {
                let l = l.borrow();
                let Some(last) = l.as_ref() else { return };
                copy_report(last);
            });
        }
        run(view, diag, last).await.map_err(|e| e.message)?;
        Ok(())
    })
}

/// The JS `drawBackups`: the Backups table with Download / Restore… per row.
fn draw_backups(bk: &Element, d: &Value) {
    let list = d.a("backups");
    if list.is_empty() {
        dom::set_html(bk, r#"<div class="empty muted small">No backups yet – press Back up now, or check the log: Encore takes one at every start when none exists.</div>"#);
    } else {
        let rows: String = list.iter().map(|b| format!(r#"<tr><td class="mono small">{name}</td><td class="small">{size}</td><td class="small">{date}</td><td><span class="badge">{kind}</span></td>
        <td><div class="row" style="justify-content:flex-end;gap:6px"><a class="btn sm" href="/api/system/backups/{href}" download="{name}">{dl}Download</a><button class="btn sm" data-restore="{name}">Restore…</button></div></td></tr>"#,
            name = esck(b, "name"), size = fmt::size(b.f("size")), date = fmt::date(b.f("time")), kind = esck(b, "kind"),
            href = String::from(js_sys::encode_uri_component(&b.s("name"))), dl = icon("download"))).collect();
        dom::set_html(bk, &format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th>Name</th><th>Size</th><th>Date</th><th>Kind</th><th></th></tr></thead><tbody>
      {rows}
    </tbody></table></div>"#));
    }
    let bk2 = bk.clone();
    dom::on_each(bk, "[data-restore]", "click", move |el, _| {
        let name = dom::data(el, "restore");
        let bk = bk2.clone();
        dom::spawn(async move {
            let ok = confirm_dialog("Restore this backup?", &format!("Encore replaces its current database with {name}. A safety copy of the current data is taken first, then the server reopens the database: everything saved since that backup (library index, requests, playlists, settings and sign-ins) is replaced, and this page reloads – you may have to sign in again."), "Restore", true).await;
            if !ok { return; }
            dom::set_html(&bk, r#"<div class="empty"><span class="spinner"></span><p class="muted">Restoring – the server reopens its database, this page reloads in a moment…</p></div>"#);
            if let Err(e) = api::post_empty(&format!("/system/backups/{}/restore", String::from(js_sys::encode_uri_component(&name)))).await {
                toast(&e.message, "err");
                load_backups(&bk).await;
                return;
            }
            toast("Backup restored", "ok");
            dom::timeout(2500, dom::reload);
        });
    });
}

/// App crash report kinds (POST /api/diagnostics/client-crash), the same words in web/js.
fn crash_kind(k: &str) -> Option<&'static str> {
    Some(match k { "crash" => "Crash", "native" => "Native crash", "anr" => "Not responding", "low_memory" => "Closed for memory", "signaled" => "Killed by the system", "web" => "Web page", _ => return None })
}

/// The JS `loadCrashes`: the "App crashes" list (newest first) and the Clear button.
async fn load_crashes(view: &Element, cr: &Element) {
    let d = match api::get("/diagnostics/client-crash").await {
        Ok(d) => d,
        Err(e) => { dom::set_html(cr, &format!(r#"<div class="empty muted small">{}</div>"#, esc(&e.message))); return; }
    };
    let list = d.a("crashes");
    if let Some(b) = dom::qs_in(view, "#cr-clear") { dom::set_disabled(&b, list.is_empty()); }
    if list.is_empty() {
        dom::set_html(cr, r#"<div class="empty muted small">No app crashes reported.</div>"#);
        return;
    }
    let items: String = list.iter().map(|c| {
        let kind = c.s("kind");
        let badge = crash_kind(&kind).map(str::to_string).unwrap_or_else(|| if c.truthy("kind") { kind.clone() } else { "Crash".to_string() });
        let meta: Vec<String> = ["device", "app_version", "os", "user"].iter().filter(|k| c.truthy(k)).map(|k| c.s(k)).collect();
        let first = if c.truthy("first_line") { c.s("first_line") } else { "Crash".to_string() };
        let time = fmt::date(c.f("time"));
        // the whole report to read and copy (R1): the cut-off first line opens it, Copy takes all of it
        let full = crash_text(&first, &badge, &meta.join(" · "), &time, &or_empty(c, "text"));
        format!(r#"
      <div class="list-item" style="align-items:flex-start">
        <span class="dot err" style="margin-top:7px"></span>
        <div class="grow" style="min-width:0">
          <div class="row wrap"><b class="ellipsis"{open}>{first}</b><span class="badge">{badge}</span>{copy}</div>
          <div class="small muted">{meta} · {time}</div>
          <details><summary class="small muted" style="cursor:pointer">Full report</summary><div class="code diag-raw">{text}</div></details>
        </div>
      </div>"#,
            open = crate::details::attrs(&badge, &full), first = esc(&first), copy = crate::details::copy_button(&full),
            badge = esc(&badge), meta = esc(&meta.join(" · ")), time = time, text = esc(&or_empty(c, "text")))
    }).collect();
    dom::set_html(cr, &format!(r#"<div class="list">{items}</div>"#));
}

/// An app crash as text, for its dialog and Copy: what, where and when, then the report.
fn crash_text(first: &str, kind: &str, meta: &str, time: &str, text: &str) -> String {
    let head = [first, kind, meta, time].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join("\n");
    if text.trim().is_empty() { head } else { format!("{head}\n\n{}", text.trim_end()) }
}

/// The JS `loadBackups`.
async fn load_backups(bk: &Element) {
    match api::get("/system/backups").await {
        Ok(d) => draw_backups(bk, &d),
        Err(e) => dom::set_html(bk, &format!(r#"<div class="empty muted small">{}</div>"#, esc(&e.message))),
    }
}

/// The JS `run`: fetch the checks and draw them into `#diag` (the result is kept in `last` for the report).
async fn run(view: Element, diag: Element, last: Rc<RefCell<Option<Value>>>) -> Result<(), ApiError> {
    dom::set_html(&diag, r#"<div class="empty"><span class="spinner"></span><p class="muted">Checking…</p></div>"#);
    let mut data = api::get("/system/diagnostics").await?;
    // Encore's VPN for Soulseek, as one more connection check (older servers have no /system/vpn)
    if let Ok(v) = api::get("/system/vpn").await {
        let row = json!({ "kind": "service", "name": "VPN for Soulseek", "ok": !v.truthy("enabled") || v.g("state").as_str() == Some("up"),
            "request": if v.truthy("endpoint") { format!("WireGuard {}", v.s("endpoint")) } else { String::new() }, "detail": vpn_text(&v) });
        if let Some(a) = data.get_mut("checks").and_then(|c| c.as_array_mut()) { a.push(row); }
    }
    *last.borrow_mut() = Some(data.clone());
    remember_machine(data.g("machine"));
    if let Some(sv) = settings_view() { show_banner(&sv, data.g("machine").truthy("encore_update_available"), &data.g("machine").s("encore_latest")); }
    if let Some(c) = dom::qs_in(&view, "#copy") { dom::set_disabled(&c, false); }
    let server = server_block(data.g("machine"));
    let checks = data.a("checks");
    let bad = checks.iter().filter(|c| !c.truthy("ok")).count();
    let groups: String = KIND.iter().map(|(k, title)| {
        let g: Vec<&Value> = checks.iter().filter(|c| c.s("kind") == *k).collect();
        if g.is_empty() { return String::new(); }
        let now = dom::now_s();
        let items: String = g.iter().map(|c| check_item(c, now)).collect();
        format!(r#"
        <div class="section-head"><h2>{title}</h2></div>
        <div class="card flat"><div class="list">{items}</div></div>"#)
    }).collect();
    let rows: String = data.a("settings").iter().map(|s| format!(r#"<tr><td class="mono small">{key}</td>
          <td>{source}</td>
          <td class="small mono ellipsis" style="max-width:360px">{value}</td></tr>"#,
        key = esck(s, "key"),
        source = if s.g("source").as_str() == Some("env") { format!(r#"<span class="badge info">env {}</span>"#, esck(s, "env")) } else { r#"<span class="badge">saved in Encore</span>"#.to_string() },
        value = esc(&js_str(s, "value")),
    )).collect();
    dom::set_html(&diag, &format!(r#"
      <div class="card row" style="margin-bottom:16px">
        <span class="badge {badc}">{badl}</span>
        <span class="muted small">Encore {version} · running as uid {uid} · {time}</span>
      </div>
      {server}
      {groups}
      {folders}
      <div class="section-head"><h2>Where settings come from</h2><span class="small muted">Environment variables always win over values saved in Encore</span></div>
      <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>Setting</th><th>Source</th><th>Value</th></tr></thead><tbody>
        {rows}
      </tbody></table></div></div>"#,
        badc = if bad > 0 { "err" } else { "ok" },
        badl = if bad > 0 { format!("{bad} problem{}", if bad > 1 { "s" } else { "" }) } else { "All good".to_string() },
        version = esck(&data, "version"),
        uid = js_str(&data, "uid"),
        time = fmt::date(data.f("time")),
        folders = download_folders_html(data.g("download_folders"), &fmt::date),
    ));
    Ok(())
}

// ---- the download folders (backlog item 17): each folder downloads are saved in, its probe's result and time ----

/// A download folder's lines in words (Diagnostics and its copied report): (what it is, the result with a
/// note or the banner's sentence, when it was probed). `date` formats a unix time.
pub fn folder_lines(r: &Value, date: &dyn Fn(f64) -> String) -> (String, String, String) {
    let what = format!("{} – {}", r.s("label"), r.s("path"));
    let result = if r.truthy("result") { r.s("result") } else { "Not checked yet".to_string() };
    let detail = if !r.truthy("ok") && r.truthy("text") { r.s("text") }
        else if r.truthy("note") { format!("{result} – {}", r.s("note")) }
        else { result };
    let when = if r.f("checked_at") > 0.0 {
        let since = if !r.truthy("ok") && r.f("since") > 0.0 { format!(" · not writable since {}", date(r.f("since"))) } else { String::new() };
        format!("Checked {}{since}", date(r.f("checked_at")))
    } else { "Not checked yet".to_string() };
    (what, detail, when)
}

/// What the Download folders section says about the probe.
pub const FOLDERS_HELP: &str = "Probed every minute while downloads run: a file is written, flushed to the disk and removed. While nothing downloads they are looked at every 10 minutes without writing, so idle disks can spin down";

/// The Download folders section (nothing from a server without the probe).
pub fn download_folders_html(d: &Value, date: &dyn Fn(f64) -> String) -> String {
    let roots = d.a("roots");
    if !d.is_object() || roots.is_empty() { return String::new(); }
    let items: String = roots.iter().map(|r| {
        let (_, detail, when) = folder_lines(r, date);
        let ok = r.truthy("ok");
        let dot = if !ok { "err" } else if matches!(r.s("state").as_str(), "ok" | "unknown") { "ok" } else { "warn" };
        let result = if r.truthy("result") { r.s("result") } else { "Not checked yet".to_string() };
        // the badge says the result: the lines under it only what it doesn't (a note, the banner's sentence, the time)
        let detail = if detail == result { String::new() }
            else { format!(r#"<div class="small" style="color:{}">{}</div>"#, if ok { "var(--muted)" } else { "var(--err)" }, esc(&detail)) };
        let when = if r.f("checked_at") > 0.0 { format!(r#"<div class="small muted">{}</div>"#, esc(&when)) } else { String::new() };
        format!(r#"
          <div class="list-item" style="align-items:flex-start">
            <span class="dot {dot}" style="margin-top:7px"></span>
            <div class="grow" style="min-width:0">
              <div class="row wrap"><b>{label}</b><span class="badge {badge}">{result}</span></div>
              <div class="small mono faint ellipsis">{path}</div>{detail}{when}
            </div>
          </div>"#,
            label = esck(r, "label"), path = esck(r, "path"), badge = if ok { if dot == "warn" { "warn" } else { "ok" } } else { "err" },
            result = esc(&result))
    }).collect();
    format!(r#"
        <div class="section-head"><h2>Download folders</h2><span class="small muted">{FOLDERS_HELP}</span></div>
        <div class="card flat"><div class="list">{items}</div></div>"#)
}

/// The "Server" block (0.26, `machine` in the diagnostics answer): what the owner's box looks like from inside
/// the container, as (label, value) rows in plain words – for the page and the copied report.
fn server_rows(s: &Value) -> Vec<(&'static str, String)> {
    let folder = |v: &Value| -> String {
        if !v.is_object() { return "not set".into(); }
        let mut t = if v.truthy("fs") { v.s("fs") } else { "unknown file system".to_string() };
        if v.has("network") { t.push_str(if v.truthy("network") { " (network)" } else { " (local)" }); }
        if v.truthy("path") { format!("{} · {t}", v.s("path")) } else { t }
    };
    let memory = if s.is_nullish("memory") { "unknown".to_string() } else {
        format!("{}{}", fmt::size(s.f("memory")), if s.g("memory_source").as_str() == Some("cgroup") { " (container limit)" } else { " (whole machine)" })
    };
    let readers = {
        let n = s.i("db_readers");
        match s.g("db_readers_source").as_str() {
            Some("env") => format!("{n} (ENCORE_DB_READERS)"),
            Some("network") => format!("{n} – off because the config folder is on a network file system (set ENCORE_DB_READERS to turn them on)"),
            _ => format!("{n} (default)"),
        }
    };
    vec![
        ("CPUs", if s.is_nullish("cpus") { "unknown".into() } else { js_str(s, "cpus") }),
        ("Memory", memory),
        ("Config folder", folder(s.g("config"))),
        ("Music folder", folder(s.g("music"))),
        ("Database", if s.is_nullish("database_bytes") { "unknown".into() } else { fmt::size(s.f("database_bytes")) }),
        ("Translations", if s.truthy("translations_on") { "on for at least one account".into() } else { "off for every account".into() }),
        ("Database read connections", readers),
    ]
}

/// The Server section of the page (nothing on a server that doesn't send the block).
fn server_block(s: &Value) -> String {
    if !s.is_object() { return String::new(); }
    let rows: String = server_rows(s).iter().map(|(k, v)| format!(r#"<tr><td class="small muted" style="white-space:nowrap">{k}</td><td class="small">{}</td></tr>"#, esc(v))).collect();
    format!(r#"
        <div class="section-head"><h2>Server</h2><span class="small muted">What Encore sees of the machine it runs on</span></div>
        <div class="card flat"><div class="table-wrap"><table class="table"><tbody>{rows}</tbody></table></div></div>"#)
}

/// One `.list-item` of a check group.
fn check_item(c: &Value, now: f64) -> String {
    let ok = c.truthy("ok");
    // the Soulseek session (servers since 0.29): the card's sentence, what is waiting, and the way to the card
    let soulseek = c.g("soulseek");
    let extra = if soulseek.is_object() {
        let details = super::apps::session_details(soulseek, now);
        let link = if ok { String::new() } else { r##"<a href="#/settings?s=slskd" style="color:var(--accent)">Settings → Plugins → Soulseek</a>"##.to_string() };
        let parts: Vec<String> = [esc(&details), link].into_iter().filter(|p| !p.is_empty()).collect();
        if parts.is_empty() { String::new() } else { format!(r#"<div class="small faint">{}</div>"#, parts.join(" · ")) }
    } else { String::new() };
    let detail = if soulseek.is_object() { super::apps::session_text_elsewhere(&if c.truthy("detail") { c.s("detail") } else { super::apps::session_text(soulseek) }) } else { or_empty(c, "detail") };
    let okc = if ok { "ok" } else { "err" };
    let status = if c.truthy("status") { format!(r#"<span class="badge {okc}">HTTP {}</span>"#, js_str(c, "status")) } else { String::new() };
    let ms = if !c.is_nullish("ms") { format!(r#"<span class="small faint">{} ms</span>"#, js_str(c, "ms")) } else { String::new() };
    let body = if c.truthy("body") {
        format!(r#"<details><summary class="small muted" style="cursor:pointer">Raw response</summary><div class="code diag-raw">{}</div></details>"#, esck(c, "body"))
    } else { String::new() };
    format!(r#"
          <div class="list-item" style="align-items:flex-start">
            <span class="dot {okc}" style="margin-top:7px"></span>
            <div class="grow">
              <div class="row wrap"><b>{name}</b>{status}{ms}</div>
              <div class="small mono faint ellipsis"{whole}>{request}</div>
              <div class="small" style="color:{color}">{detail}</div>
              {extra}
              {body}
            </div>
          </div>"#,
        name = esck(c, "name"),
        // the cut-off request line opens the whole check, to read and copy (R1)
        whole = if c.truthy("request") { crate::details::attrs(&c.s("name"), &[c.s("name"), or_empty(c, "request"), detail.clone(), or_empty(c, "body")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("\n")) } else { String::new() },
        request = esc(&or_empty(c, "request")),
        color = if ok { "var(--muted)" } else { "var(--err)" },
        detail = esc(&detail),
    )
}

/// The `#copy` handler: the plain-text report on the clipboard.
fn copy_report(last: &Value) {
    let mut lines = vec![format!("Encore {} diagnostics (uid {})", js_str(last, "version"), js_str(last, "uid"))];
    for c in last.a("checks") {
        let ok = c.truthy("ok");
        let status = if c.truthy("status") { format!(" → HTTP {}", js_str(&c, "status")) } else { String::new() };
        let ms = if !c.is_nullish("ms") { format!(" ({} ms)", js_str(&c, "ms")) } else { String::new() };
        // `c.body.slice(0, 300)` – JS slices by UTF-16 units
        let body = if c.truthy("body") && !ok { format!("\n     body: {}", String::from(js_sys::JsString::from(c.s("body")).slice(0, 300))) } else { String::new() };
        lines.push(format!("{} {} – {}{status}{ms}\n     {}{body}", if ok { "OK  " } else { "FAIL" }, js_str(&c, "name"), or_empty(&c, "request"), or_empty(&c, "detail")));
    }
    let folders = last.g("download_folders").a("roots");
    if !folders.is_empty() {
        lines.push(String::new());
        lines.push("Download folders:".into());
        for r in &folders {
            let (what, detail, when) = folder_lines(r, &fmt::date);
            lines.push(format!("{} {what}\n     {detail} ({when})", if r.truthy("ok") { "OK  " } else { "FAIL" }));
        }
    }
    if last.g("machine").is_object() {
        lines.push(String::new());
        lines.push("Server:".into());
        for (k, v) in server_rows(last.g("machine")) {
            lines.push(format!("  {k}: {v}"));
        }
    }
    lines.push(String::new());
    lines.push("Settings:".into());
    for s in last.a("settings") {
        lines.push(format!("  {} = {}  [{}]", js_str(&s, "key"), js_str(&s, "value"), if s.g("source").as_str() == Some("env") { format!("env {}", js_str(&s, "env")) } else { "saved".to_string() }));
    }
    copy_text(&lines.join("\n"));
}

// ---- Updates (0.28): the version this server runs, the newest release, Check now, Restart server, Update now ----

/// The README section on "Update now" (the Docker socket mount) and "Restart server".
const README_UPDATES: &str = "https://github.com/Krifmaz/Encore#updates-and-restart";
/// After Restart server / Update now: /api/health every 2 s; after 90 s the page says the server is late (and
/// keeps looking for 10 minutes in all).
const POLL_MS: u32 = 2000;
const BACK_BUDGET_MS: f64 = 90_000.0;
const GIVE_UP_MS: f64 = 600_000.0;
/// A restart so quick that no poll saw the server down: this long of answers in a row counts as back.
const QUIET_BACK_MS: f64 = 30_000.0;
/// The Settings banner reads the diagnostics answer at most this often per page load.
const MACHINE_FRESH_MS: f64 = 30.0 * 60.0 * 1000.0;

thread_local! {
    /// The last `machine` block of /api/system/diagnostics and when it came (ms), for the Settings banner.
    static MACHINE: RefCell<Option<(f64, Value)>> = const { RefCell::new(None) };
}

fn remember_machine(m: &Value) {
    if m.is_object() { MACHINE.with(|x| *x.borrow_mut() = Some((dom::now_ms(), m.clone()))); }
}

/// Release notes (GitHub markdown) as plain lines for "What changed": headings lose their #, list items get a
/// bullet, * and ` go, [text](link) and ![text](image) keep their text, <https://…> its address; HTML
/// comments, code fences, rules and runs of blank lines go.
pub fn notes_text(md: &str) -> String {
    let mut text = md.replace("\r\n", "\n");
    while let Some(a) = text.find("<!--") {
        let end = text[a..].find("-->").map(|e| a + e + 3).unwrap_or(text.len());
        text.replace_range(a..end, "");
    }
    let mut out: Vec<String> = vec![];
    for raw in text.lines() {
        let body = raw.trim_start();
        let indent = (raw.len() - body.len()).min(8);
        let mut t = body.trim_end();
        if t.starts_with("```") || t.starts_with("~~~") { continue; }
        if t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) && t.chars().filter(|c| *c != ' ').count() >= 3 {
            t = "";
        }
        if t.starts_with('#') { t = t.trim_start_matches('#').trim_start(); }
        while let Some(r) = t.strip_prefix('>') { t = r.trim_start(); }
        let mut bullet = false;
        for m in ["* ", "- ", "+ "] {
            if let Some(r) = t.strip_prefix(m) { t = r.trim_start(); bullet = true; break; }
        }
        let line = inline_text(t);
        if line.trim().is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) { out.push(String::new()); }
            continue;
        }
        out.push(format!("{}{}{}", " ".repeat(indent), if bullet { "• " } else { "" }, line.trim_end()));
    }
    while out.last().is_some_and(|l| l.is_empty()) { out.pop(); }
    out.join("\n")
}

/// `[text](url)` at `i` (the `[`): its text and the index after the `)`.
fn md_link(c: &[char], i: usize) -> Option<(String, usize)> {
    let close = (i + 1..c.len()).find(|&j| c[j] == ']')?;
    if c.get(close + 1) != Some(&'(') { return None; }
    let end = (close + 2..c.len()).find(|&j| c[j] == ')')?;
    Some((c[i + 1..close].iter().collect(), end + 1))
}

fn inline_text(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let at = if c[i] == '!' && c.get(i + 1) == Some(&'[') { i + 1 } else { i };
        if c[at] == '[' {
            if let Some((text, next)) = md_link(&c, at) {
                out.push_str(&inline_text(&text));
                i = next;
                continue;
            }
        }
        if c[i] == '<' {
            if let Some(end) = (i + 1..c.len()).find(|&j| c[j] == '>') {
                let inner: String = c[i + 1..end].iter().collect();
                if inner.starts_with("http://") || inner.starts_with("https://") {
                    out.push_str(&inner);
                    i = end + 1;
                    continue;
                }
            }
        }
        if c[i] != '*' && c[i] != '`' { out.push(c[i]); }
        i += 1;
    }
    out
}

/// GitHub's `published_at` (an ISO date) as "Oct 12, 2026" in the reader's language ("" when missing).
fn published_day(v: &Value) -> String {
    let Some(s) = v.as_str().filter(|s| !s.is_empty()) else { return String::new() };
    let ms = js_sys::Date::parse(s);
    if ms.is_nan() { return String::new(); }
    fmt::day(ms / 1000.0)
}

/// The Updates card: GET /system/update (`?force=1` for Check now) and draw it into `up`.
async fn load_updates(up: &Element, force: bool) {
    let path = if force { "/system/update?force=1" } else { "/system/update" };
    match api::get(path).await {
        Ok(d) => {
            note_update(&d);
            draw_updates(up, &d);
        }
        // not an admin, or a server without the endpoint: nothing to show
        Err(e) if e.status == 403 || e.status == 404 => dom::set_html(up, ""),
        Err(e) => {
            dom::set_html(up, &format!(r#"<div class="section-head"><h2>Updates</h2></div>
    <div class="card"><div class="small" style="color:var(--err)">{}</div></div>"#, esc(&e.message)));
        }
    }
}

fn updates_html(d: &Value) -> String {
    let current = d.s("current");
    let latest = d.s("latest");
    let newer = d.truthy("is_newer") && !latest.is_empty();
    // a build without a release number (cargo build, or the image built from the sources)
    let dev = d.truthy("dev");
    let dev_note = if dev { r#"<div class="small muted">A build without a release number: any release counts as newer.</div>"# } else { "" };
    let error = if d.truthy("error") { format!(r#"<div class="small" style="color:var(--err)">Couldn't check for updates: {}</div>"#, esck(d, "error")) } else { String::new() };
    let checked = if d.f("checked_at") > 0.0 { format!(r#"<span class="small faint">checked {}</span>"#, esc(&fmt::ago(d.f("checked_at")))) } else { String::new() };
    let state = if latest.is_empty() {
        if error.is_empty() { r#"<span class="small muted">Not checked yet – press Check now</span>"#.to_string() } else { String::new() }
    } else if newer {
        let day = published_day(d.g("published_at"));
        format!(r#"<span class="badge info">Encore {}{} is available</span>{}"#, esc(&latest), if d.truthy("hotfix") { " (hotfix)" } else { "" },
            if day.is_empty() { String::new() } else { format!(r#"<span class="small muted">released {}</span>"#, esc(&day)) })
    } else {
        r#"<span class="small muted">You have the newest version</span>"#.to_string()
    };
    let github = if d.truthy("url") {
        format!(r#"<a class="btn sm" href="{}" target="_blank" rel="noopener">{}View on GitHub</a>"#, esck(d, "url"), icon("external"))
    } else { String::new() };
    let hotfix = d.g("channel").as_str() == Some("hotfix");
    let channel = format!(r#"<select class="input" id="up-channel" style="width:auto;padding:6px 10px" title="Stable: full releases only. Hotfix: also the build of every push to GitHub; a full release still wins when it is newer.">
        <option value="stable"{}>Stable releases</option><option value="hotfix"{}>Hotfixes (every push)</option></select>"#,
        if hotfix { "" } else { " selected" }, if hotfix { " selected" } else { "" });
    let ready = d.g("self_update").as_str() == Some("ready");
    let update_btn = if ready && newer { r#"<button class="btn sm primary" id="up-now">Update now</button>"#.to_string() } else { String::new() };
    let notes = notes_text(&d.s("notes"));
    let notes = if notes.is_empty() { String::new() } else {
        format!(r#"<details><summary class="small muted" style="cursor:pointer">What changed in {}</summary><div class="small" style="white-space:pre-wrap;margin-top:8px">{}{}</div></details>"#,
            esc(&latest), esc(&notes), if d.truthy("notes_cut") { "\n…" } else { "" })
    };
    let help = if ready { String::new() } else {
        let reason = if d.truthy("self_update_reason") { d.s("self_update_reason") } else { "The Docker socket is not mounted".to_string() };
        format!(r#"<div class="small muted">Update now is not available here: {}. <a href="{README_UPDATES}" target="_blank" rel="noopener" style="color:var(--accent)">How to turn it on</a></div>"#,
            esc(reason.trim_end_matches('.')))
    };
    let shell = crate::sw::shell_cache().map(|c| format!(r#"<div class="small faint">This browser keeps the web app for offline use ({})</div>"#, esc(&c))).unwrap_or_default();
    format!(r#"
    <div class="section-head"><h2>Updates</h2>
      <div class="row">{channel}<button class="btn sm" id="up-check">{refresh}Check now</button><button class="btn sm" id="up-restart">Restart server</button></div>
    </div>
    <div class="card stack">
      <div class="row wrap between">
        <div class="stack" style="gap:6px">
          <div style="font-weight:700;font-size:20px">Encore {current}{dev}</div>
          <div class="row wrap" style="gap:8px">{state}{checked}</div>
        </div>
        <div class="row wrap">{github}{update_btn}</div>
      </div>
      {dev_note}{error}{notes}{help}{shell}
      <div id="up-status" hidden></div>
    </div>"#, refresh = icon("refresh"), current = esc(&current), dev = if dev { " · development build" } else { "" })
}

fn draw_updates(up: &Element, d: &Value) {
    dom::set_html(up, &updates_html(d));
    {
        // the channel is the update_channel setting; a change checks GitHub again for that channel
        let up2 = up.clone();
        dom::on_sel(up, "#up-channel", "change", move |e| {
            let Some(sel) = dom::target_el(e) else { return };
            let (up, channel) = (up2.clone(), dom::value(&sel));
            dom::spawn(async move {
                match api::put("/admin/settings", json!({ "update_channel": channel })).await {
                    Ok(_) => load_updates(&up, true).await,
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
    }
    {
        let up2 = up.clone();
        dom::on_sel(up, "#up-check", "click", move |e| {
            let Some(btn) = dom::target_el(e).and_then(|t| dom::closest_of(&t, "button")) else { return };
            let up = up2.clone();
            dom::set_disabled(&btn, true);
            dom::set_html(&btn, r#"<span class="spinner" style="width:14px;height:14px"></span>Checking…"#);
            dom::spawn(async move { load_updates(&up, true).await });
        });
    }
    {
        let up2 = up.clone();
        let current = d.s("current");
        dom::on_sel(up, "#up-restart", "click", move |_| {
            let (up, current) = (up2.clone(), current.clone());
            dom::spawn(async move {
                if !confirm_dialog("Restart the server?", "Playback stops for about ten seconds.", "Restart", true).await { return; }
                busy(&up, true);
                status(&up, &format!(r#"<span class="spinner"></span><span>{}</span>"#, "Restarting – waiting for the server…"));
                if let Err(e) = api::post_empty("/system/restart").await {
                    toast(&e.message, "err");
                    status_off(&up);
                    busy(&up, false);
                    return;
                }
                wait_back(&up, Wait::Restart, &current).await;
            });
        });
    }
    {
        let up2 = up.clone();
        let (current, latest) = (d.s("current"), d.s("latest"));
        dom::on_sel(up, "#up-now", "click", move |_| {
            let (up, current, latest) = (up2.clone(), current.clone(), latest.clone());
            dom::spawn(async move {
                let ok = confirm_dialog(&format!("Update to {latest}?"), "The container is replaced by a new one with the same settings; this takes a minute.", "Update now", false).await;
                if !ok { return; }
                busy(&up, true);
                status(&up, &format!(r#"<span class="spinner"></span><span>Downloading Encore {} – this can take a few minutes…</span>"#, esc(&latest)));
                match api::post("/system/update", json!({ "tag": latest })).await {
                    Ok(r) => {
                        let msg = if r.truthy("message") { r.s("message") } else { format!("Updating to {latest}; the server restarts in a moment") };
                        status(&up, &format!(r#"<span class="spinner"></span><span>{}</span>"#, esc(&msg)));
                        wait_back(&up, Wait::Update, &current).await;
                    }
                    Err(e) => {
                        status(&up, &format!(r#"<span class="dot err"></span><span style="color:var(--err)">{}</span>"#, esc(&e.message)));
                        busy(&up, false);
                    }
                }
            });
        });
    }
}

/// The buttons of the card while the server restarts.
fn busy(up: &Element, on: bool) {
    for sel in ["#up-channel", "#up-check", "#up-restart", "#up-now"] {
        if let Some(b) = dom::qs_in(up, sel) { dom::set_disabled(&b, on); }
    }
}

fn status(up: &Element, html: &str) {
    if let Some(s) = dom::qs_in(up, "#up-status") {
        dom::set_html(&s, &format!(r#"<div class="row wrap small">{html}</div>"#));
        dom::hide(&s, false);
    }
}

fn status_off(up: &Element) {
    if let Some(s) = dom::qs_in(up, "#up-status") { dom::set_html(&s, ""); dom::hide(&s, true); }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait { Restart, Update }

/// What the page makes of the polls so far.
#[derive(Debug, PartialEq, Eq)]
enum Back { Waiting, Running(String), SameVersion, Late }

/// The decision of `wait_back`, apart from the network: `down` once a poll failed, `answering_for` how long the
/// server has answered without a failure, `version` what /api/health said (None: no answer).
fn judge(w: Wait, from: &str, version: Option<&str>, down: bool, answering_for: f64, elapsed: f64) -> Back {
    match (w, version) {
        (Wait::Restart, Some(v)) if down || answering_for >= QUIET_BACK_MS => Back::Running(v.to_string()),
        (Wait::Update, Some(v)) if v != from => Back::Running(v.to_string()),
        (Wait::Update, Some(_)) if down => Back::SameVersion,
        _ if elapsed >= BACK_BUDGET_MS => Back::Late,
        _ => Back::Waiting,
    }
}

/// Poll /api/health every 2 s until the server is back (a restart: it went away and answers again; an update:
/// it answers with another version), then show "Back" with Reload. After 90 s the card says the server is late
/// and keeps looking for 10 minutes in all.
async fn wait_back(up: &Element, w: Wait, from: &str) {
    let start = dom::now_ms();
    let mut down = false;
    let mut answering_since: Option<f64> = None;
    let mut late = false;
    loop {
        dom::sleep(POLL_MS).await;
        if !up.is_connected() { return; }
        let h = crate::update::health().await;
        let now = dom::now_ms();
        let version = h.as_ref().map(|v| v.s("version"));
        match &version {
            None => { down = true; answering_since = None; }
            Some(_) => { answering_since.get_or_insert(now); }
        }
        let answering_for = answering_since.map(|t| now - t).unwrap_or(0.0);
        match judge(w, from, version.as_deref(), down, answering_for, now - start) {
            Back::Running(v) => {
                status(up, &format!(r#"<span class="dot ok"></span><b>Back</b><span class="muted">Encore {} is running</span><button class="btn sm primary" id="up-reload">Reload</button>"#, esc(&v)));
                if let Some(b) = dom::qs_in(up, "#up-reload") { dom::on(&b, "click", |_| dom::reload()); }
                return;
            }
            Back::SameVersion => {
                status(up, &format!(r#"<span class="dot err"></span><span>The server is back, still on Encore {}: the update did not go through. Try Update now again, or update by hand (<code>docker compose pull && docker compose up -d</code>).</span><button class="btn sm" id="up-reload">Reload</button>"#, esc(from)));
                if let Some(b) = dom::qs_in(up, "#up-reload") { dom::on(&b, "click", |_| dom::reload()); }
                return;
            }
            Back::Late if !late => {
                late = true;
                status(up, r#"<span class="spinner"></span><span>The server did not come back yet – check <code>docker ps</code></span>"#);
            }
            _ => {}
        }
        if now - start >= GIVE_UP_MS {
            status(up, r#"<span class="dot err"></span><span>The server did not come back yet – check <code>docker ps</code></span><button class="btn sm" id="up-reload">Reload</button>"#);
            if let Some(b) = dom::qs_in(up, "#up-reload") { dom::on(&b, "click", |_| dom::reload()); }
            return;
        }
    }
}

/// A fresh answer of /system/update also tells the Settings banner (same screen) whether to show.
fn note_update(d: &Value) {
    let available = d.truthy("is_newer") && d.truthy("latest");
    MACHINE.with(|m| {
        if let Some((_, v)) = m.borrow_mut().as_mut() {
            if let Some(o) = v.as_object_mut() {
                o.insert("encore_update_available".into(), json!(available));
                o.insert("encore_latest".into(), d.g("latest").clone());
            }
        }
    });
    if let Some(view) = settings_view() { show_banner(&view, available, &d.s("latest")); }
}

/// The Settings page around the Diagnostics panel (its title takes the banner).
fn settings_view() -> Option<Element> { dom::qs(".settings-grid").and_then(|g| g.parent_element()) }

/// Settings: the line under the title for admins when the server's last check found a newer Encore ("Encore
/// 0.29.0 is available – Diagnostics"). It reads the `machine` block of /api/system/diagnostics, where the
/// server keeps its last check (nothing goes to GitHub), at most once per half hour per page load; the
/// Diagnostics page refreshes it whenever it runs.
pub fn update_banner(view: &Element) {
    if !crate::app::is_admin() { return; }
    // opened on Diagnostics: its own run of the checks fills the banner
    if dom::hash().contains("s=diagnostics") { return; }
    let view = view.clone();
    dom::spawn(async move {
        let cached = MACHINE.with(|m| m.borrow().as_ref().filter(|(t, _)| dom::now_ms() - t < MACHINE_FRESH_MS).map(|(_, v)| v.clone()));
        let m = match cached {
            Some(m) => m,
            None => match api::get("/system/diagnostics").await {
                Ok(d) => { remember_machine(d.g("machine")); d.g("machine").clone() }
                Err(_) => return,
            },
        };
        if !view.is_connected() { return; }
        show_banner(&view, m.truthy("encore_update_available"), &m.s("encore_latest"));
    });
}

fn show_banner(view: &Element, available: bool, latest: &str) {
    if let Some(b) = dom::by_id("update-banner") { b.remove(); }
    if !available || latest.is_empty() { return; }
    let Some(h1) = dom::qs_in(view, "h1") else { return };
    let _ = h1.insert_adjacent_html("afterend", &format!(r##"<div id="update-banner" class="card row" style="margin:-6px 0 16px;padding:10px 14px"><span class="badge info">Update</span><span class="small">Encore {} is available – <a href="#/settings?s=diagnostics" style="color:var(--accent)">Diagnostics</a></span></div>"##, esc(latest)));
    if let Some(a) = dom::qs("#update-banner a") {
        dom::on(&a, "click", |e| {
            // switch the panel like the side menu does (the page stays); without the menu the link reloads it
            if let Some(b) = dom::qs(r#".settings-nav [data-s="diagnostics"]"#) { dom::prevent(e); dom::click(&b); }
        });
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn notes_become_plain_lines() {
        let md = "## What's new\r\n\r\n* **Offline**: songs play without the server ([docs](https://x.y/docs))\n- Update from `Settings`\n\n\n\n### Fixes\n  * a nested ~~one~~ item\n> quoted *line*\n![shot](https://x.y/a.png)\n<!-- hidden\nnote -->\n---\n```sh\ndocker compose pull\n```\nSee <https://github.com/Krifmaz/Encore>.";
        assert_eq!(notes_text(md), "What's new\n\n• Offline: songs play without the server (docs)\n• Update from Settings\n\nFixes\n  • a nested ~~one~~ item\nquoted line\nshot\n\ndocker compose pull\nSee https://github.com/Krifmaz/Encore.");
    }

    #[test]
    fn notes_keep_plain_text_and_odd_brackets() {
        assert_eq!(notes_text(""), "");
        assert_eq!(notes_text("\n\n# Title\n\n"), "Title");
        assert_eq!(notes_text("a [b] c (d) e!"), "a [b] c (d) e!");
        assert_eq!(notes_text("x < y and [unclosed](link"), "x < y and [unclosed](link");
        assert_eq!(notes_text("[a](u1) and [b](u2)"), "a and b");
        assert_eq!(notes_text("Ünïcode – “quotes” [ok](u)"), "Ünïcode – “quotes” ok");
    }

    #[test]
    fn the_wait_after_a_restart_or_an_update() {
        // a restart: back once the server went away and answers again
        assert_eq!(judge(Wait::Restart, "0.28.0", Some("0.28.0"), false, 0.0, 2000.0), Back::Waiting);
        assert_eq!(judge(Wait::Restart, "0.28.0", None, true, 0.0, 4000.0), Back::Waiting);
        assert_eq!(judge(Wait::Restart, "0.28.0", Some("0.28.0"), true, 0.0, 6000.0), Back::Running("0.28.0".into()));
        // so quick that no poll saw it down: 30 s of answers count as back
        assert_eq!(judge(Wait::Restart, "0.28.0", Some("0.28.0"), false, QUIET_BACK_MS, 32_000.0), Back::Running("0.28.0".into()));
        // an update: back when another version answers; the old one after a gap means it did not go through
        assert_eq!(judge(Wait::Update, "0.28.0", Some("0.28.0"), false, 40_000.0, 40_000.0), Back::Waiting);
        assert_eq!(judge(Wait::Update, "0.28.0", Some("0.29.0"), false, 0.0, 8000.0), Back::Running("0.29.0".into()));
        assert_eq!(judge(Wait::Update, "0.28.0", Some("0.28.0"), true, 0.0, 30_000.0), Back::SameVersion);
        // late after 90 s
        assert_eq!(judge(Wait::Update, "0.28.0", None, true, 0.0, BACK_BUDGET_MS), Back::Late);
        assert_eq!(judge(Wait::Restart, "0.28.0", None, true, 0.0, BACK_BUDGET_MS + 2000.0), Back::Late);
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn each_download_folder_with_its_result_and_time() {
        let date = |t: f64| format!("t{t}");
        let d = json!({ "ok": false, "text": "x", "roots": [
            { "key": "staging_path", "label": "Staging folder (yt-dlp, streamrip, links)", "path": "/downloads/encore", "state": "read_only", "result": "Read-only", "ok": false,
              "text": "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount", "checked_at": 20.0, "since": 10.0 },
            { "key": "qbit_download_path", "label": "Torrent downloads (qBittorrent)", "path": "/downloads/torrents", "state": "denied", "result": "Permission denied", "ok": true,
              "note": "Encore may not write here, which is fine", "checked_at": 20.0, "since": null },
            { "key": "sab_download_path", "label": "Usenet downloads (SABnzbd)", "path": "/downloads/usenet", "state": "unknown", "result": "Not checked yet", "ok": true, "checked_at": null } ] });
        let roots = d.a("roots");
        assert_eq!(folder_lines(&roots[0], &date), ("Staging folder (yt-dlp, streamrip, links) – /downloads/encore".to_string(),
            "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount".to_string(), "Checked t20 · not writable since t10".to_string()));
        assert_eq!(folder_lines(&roots[1], &date).1, "Permission denied – Encore may not write here, which is fine");
        assert_eq!(folder_lines(&roots[1], &date).2, "Checked t20");
        assert_eq!(folder_lines(&roots[2], &date).2, "Not checked yet");
        let html = download_folders_html(&d, &date);
        assert!(html.contains("<h2>Download folders</h2>"), "{html}");
        assert!(html.contains(r#"<span class="dot err""#) && html.contains(r#"<span class="dot warn""#) && html.contains(r#"<span class="dot ok""#), "{html}");
        assert!(html.contains(r#"<span class="badge err">Read-only</span>"#), "{html}");
        assert_eq!(download_folders_html(&Value::Null, &date), "", "an older server");
        // the result is said once: in the badge, not again under the path; no time before the first probe
        assert_eq!(html.matches("Not checked yet").count(), 1, "{html}");
        let fine = json!({ "roots": [{ "key": "staging_path", "label": "Staging folder", "path": "/downloads/encore", "state": "ok", "result": "Writable", "ok": true, "checked_at": 20.0 }] });
        let html = download_folders_html(&fine, &date);
        assert_eq!(html.matches("Writable").count(), 1, "{html}");
        assert!(html.contains("Checked t20") && html.contains(FOLDERS_HELP), "{html}");
    }
}
