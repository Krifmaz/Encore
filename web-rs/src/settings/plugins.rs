//! Settings → Server → Plugins (0.32): the catalogue of what this server can use and what is installed.
//!
//! GET /api/plugins lists every plugin (the catalogue first, then the plugins added from a zip, an address or
//! GitHub) with the sandbox level. Each installed plugin is a card: its kind, version, status line, the buttons
//! (Update, Test, Logs, Disable or Enable, Remove, and the auto-update switch), a logs drawer, and its settings,
//! drawn from its manifest (GET /api/plugins/{id}/settings, fields in the settings schema's shape) by the same
//! renderer as the other Server cards (schema_form.rs). The plugins that are not installed are listed under
//! Available with Install. "Add a plugin" uploads a zip (multipart, `api::post_form`) or installs from an https
//! address or `owner/repo@ref`, after the third-party warning.
//!
//! The source cards of the Downloads page moved here in 0.32 (streamrip, Torrents & Usenet, Soulseek, yt-dlp
//! with YouTube sign-in as a part of its card), with their extras from apps.rs: Tidal sign-in and the streamrip
//! config, the Soulseek status and VPN block, the YouTube sign-in block. A built-in plugin's settings are
//! Encore's own keys, so they are read from and saved with GET/PUT /admin/settings as before (env locks, the
//! eye and copy buttons, the folder warnings); a plugin's own settings go to PUT /api/plugins/{id}/settings.
//! Old `#/settings?s=streamrip` (… torznab, slskd, ytdlp, youtube) links open this page with that card.

use super::fields::{Field, Group, Schema};
use super::schema_form::{bind_show_when, form_html, form_values};
use super::{apps, groups, test_result, Hub};
use crate::api::{self, ApiError};
use crate::dom;
use crate::json::{js_string, json, J};
use crate::ui::{copy_text, esc, icon, toast};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement};

/// The cards of the Downloads page that became plugin cards in 0.32: (card, plugin). The YouTube sign-in card is
/// a part of yt-dlp's card.
pub const MOVED: &[(&str, &str)] = &[
    ("streamrip", "streamrip"), ("torznab", "torznab"), ("slskd", "slskd"), ("ytdlp", "ytdlp"), ("youtube", "ytdlp"),
];

/// Other `?s=` names that open the Plugins page: the page itself, the old Download tools card, and the catalogue
/// plugins whose settings are on no other page.
const LINKS: &[&str] = &[
    "plugins", "tools", "qbittorrent", "transmission", "deluge", "rtorrent", "torrent-blackhole", "sabnzbd", "nzbget",
    "usenet-blackhole", "translator", "jellyfin", "plex", "navidrome",
];

/// The download client plugins, by the select of the Torrents & Usenet card that chooses them: (select, value,
/// plugin).
pub const CLIENTS: &[(&str, &str, &str)] = &[
    ("torrent_client", "qbittorrent", "qbittorrent"), ("torrent_client", "transmission", "transmission"),
    ("torrent_client", "deluge", "deluge"), ("torrent_client", "rtorrent", "rtorrent"), ("torrent_client", "blackhole", "torrent-blackhole"),
    ("nzb_client", "sabnzbd", "sabnzbd"), ("nzb_client", "nzbget", "nzbget"), ("nzb_client", "blackhole", "usenet-blackhole"),
];

/// Whether a card of the settings schema is drawn on the Plugins page instead of its own page.
pub fn moved(card: &str) -> bool { MOVED.iter().any(|(c, _)| *c == card) }

/// Whether `#/settings?s=<s>` opens the Plugins page.
pub fn opens_plugins(s: &str) -> bool { moved(s) || LINKS.contains(&s) }

/// The plugin card a `?s=` name asks for: a moved card's plugin, or the name itself.
pub fn card_for(s: &str) -> &str { MOVED.iter().find(|(c, _)| *c == s).map(|(_, p)| *p).unwrap_or(s) }

// ---- words --------------------------------------------------------------------------------------------------

/// A plugin's kind in a badge.
pub fn kind_label(kind: &str) -> String {
    match kind {
        "source" => "Source".into(),
        "client" => "Download client".into(),
        "tool" => "Tool".into(),
        "playback" => "Playback".into(),
        "connection" => "Connection".into(),
        "" => "Plugin".into(),
        k => { let mut c = k.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }
    }
}

/// "songs, albums and links": what a source finds (`kinds`).
pub fn kinds_words(kinds: &[Value]) -> String {
    let words: Vec<&str> = kinds.iter().filter_map(|k| match k.as_str() { Some("track") => Some("songs"), Some("album") => Some("albums"), Some("url") => Some("links"), _ => None }).collect();
    match words.len() {
        0 => String::new(),
        1 => words[0].to_string(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// The version a card shows: the installed one (the program's), else the plugin's.
pub fn version_text(c: &Value) -> String {
    if c.truthy("installed") && c.truthy("installed_version") { c.s("installed_version") } else { c.s("version") }
}

/// What the page is doing to a plugin, in a few words.
fn busy_words(what: &str) -> &'static str {
    match what {
        "install" | "installing" => "Installing…",
        "update" | "updating" => "Updating…",
        "remove" | "removing" => "Removing…",
        "enable" => "Enabling…",
        "disable" => "Disabling…",
        "test" => "Testing…",
        _ => "Working…",
    }
}

/// The warning before a plugin from a zip, an address or GitHub is installed, for the sandbox level here.
pub fn trust_text(full: bool) -> &'static str {
    if full {
        "This plugin is not made by Encore. It runs in full isolation: it sees only its own folders, and the network only when its plugin.toml asks for it. Install only plugins you trust."
    } else {
        "This plugin is not made by Encore. This server can only set limits on it (see the compose option in the README): it runs as Encore's user and can read what Encore can. Install only plugins you trust."
    }
}

/// The toast after an action on a plugin (`before` and `after`: its card).
pub fn done_words(act: &str, before: &Value, after: &Value) -> String {
    let name = if after.truthy("name") { after.s("name") } else { before.s("name") };
    let v = version_text(after);
    match act {
        "install" => if after.truthy("installed_version") { format!("{name} {v} installed") } else { format!("{name} installed") },
        "update" => {
            if before.s("installed_version") == after.s("installed_version") { format!("{name} is up to date") } else { format!("{name} updated to {v}") }
        }
        "remove" => format!("{name} removed"),
        "enable" => format!("{name} enabled"),
        "disable" => format!("{name} disabled"),
        _ => "Done".into(),
    }
}

/// A failed call as one text: the server's sentence and, for a refused manifest, each problem on its own line.
pub fn error_text(message: &str, data: &Value) -> String {
    let problems: Vec<String> = data.a("problems").iter().map(js_string).filter(|p| !p.is_empty()).collect();
    let head = if message.trim().is_empty() { "Failed".to_string() } else { message.to_string() };
    if problems.is_empty() { head } else { format!("{head}\n{}", problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")) }
}

/// A failure in full, readable and with Copy (R1).
pub fn error_html(text: &str) -> String {
    format!(r##"<div class="test-result err">{}<span style="white-space:pre-wrap">{}</span>{}</div>"##, icon("x"), esc(text), crate::details::copy_button(text))
}

/// What a Test answered (`{ok, message}` or `{ok: false, error, code, retry_after}`), as (ok, words).
pub fn test_answer(r: &Value) -> (bool, String) {
    let ok = r.truthy("ok");
    let mut msg = if r.truthy("message") { r.s("message") } else { r.s("error") };
    if !ok && r.s("code") == "rate_limited" && r.f("retry_after") > 0.0 {
        msg = format!("{msg} – try again in {}", apps::span(r.f("retry_after")));
    }
    if msg.trim().is_empty() { msg = if ok { "Test passed".into() } else { "Test failed".into() }; }
    (ok, msg)
}

/// A Test answer as the cards show one (a failure with Copy).
pub fn test_html(r: &Value) -> String {
    let (ok, message) = test_answer(r);
    test_result(&json!({ "ok": ok, "message": message }))
}

/// Why an address for Install from URL cannot be used, before it is sent.
pub fn url_problem(input: &str) -> Option<&'static str> {
    let s = input.trim();
    if s.is_empty() { return Some("Enter an https address, or owner/repo@ref for GitHub"); }
    if s.to_ascii_lowercase().starts_with("http://") { return Some("Plugins install only from https addresses"); }
    None
}

/// Why a picked file cannot be uploaded, before it is sent.
pub fn file_problem(name: &str, bytes: f64, max_mb: i64) -> Option<String> {
    if !name.to_ascii_lowercase().ends_with(".zip") { return Some("A plugin is uploaded as a .zip file".into()); }
    if bytes > (max_mb.max(1) as f64) * 1024.0 * 1024.0 { return Some(format!("The plugin zip is larger than {max_mb} MB")); }
    None
}

/// An address the page may link to: http(s) only (a plugin's homepage is the plugin's own text).
fn safe_link(u: &str) -> bool { let l = u.trim().to_ascii_lowercase(); l.starts_with("https://") || l.starts_with("http://") }

// ---- the parts of a card ------------------------------------------------------------------------------------

const SPIN: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;

/// The sandbox line at the top of the page (`sandbox` of GET /api/plugins).
pub fn sandbox_html(sb: &Value) -> String {
    let full = sb.s("level") == "full";
    let text = if sb.truthy("text") { sb.s("text") } else if full { "Full isolation".into() } else { "Limits only, see the compose option".into() };
    let what = if full {
        "Plugins you add see only their own folders, and the network only when they ask for it."
    } else {
        "Plugins you add run as Encore's user with limits on memory, processor time and processes; the compose option in the README gives them full isolation."
    };
    let reason = sb.s("reason");
    let reason = if reason.trim().is_empty() { String::new() } else {
        format!(r##"<div class="small faint ellipsis" style="width:100%"{}>{}</div>"##, crate::details::attrs("Sandbox", &reason), esc(&reason))
    };
    format!(r##"<div class="row wrap small" style="gap:8px;align-items:center" id="plugins-sandbox">{shield}<b>Sandbox</b><span class="badge {cls}">{text}</span><span class="muted">{what}</span>{reason}</div>"##,
        shield = crate::ui::icon_attr("shield", r#"width="16" height="16" style="flex:none""#), cls = if full { "ok" } else { "warn" }, text = esc(&text))
}

/// The badges of a card or row: kind, third-party, off, an update.
pub fn badges_html(c: &Value) -> String {
    let mut b = vec![format!(r##"<span class="badge">{}</span>"##, esc(&kind_label(&c.s("kind"))))];
    if c.s("trust") == "third-party" || (c.s("origin") != "catalog" && c.truthy("origin")) { b.push(r##"<span class="badge warn">Third-party</span>"##.into()); }
    if c.truthy("installed") && !c.truthy("enabled") { b.push(r##"<span class="badge">Off</span>"##.into()); }
    if c.truthy("installed") && c.truthy("update_available") && c.truthy("latest") {
        b.push(format!(r##"<span class="badge info">{} available</span>"##, esc(&c.s("latest"))));
    } else if c.truthy("installed") && c.truthy("latest") {
        b.push(format!(r##"<span class="badge ok">{} Up to date</span>"##, crate::ui::icon_attr("check", r#"width="12" height="12""#)));
    }
    b.concat()
}

/// The dot beside an installed plugin's name: ok, a problem, needs setting up (it works but isn't signed in or
/// has no address yet: `status.signed_in` false), or off.
pub fn dot_html(c: &Value) -> String {
    if !c.truthy("installed") { return String::new(); }
    let st = c.g("status");
    let tone = if !c.truthy("enabled") { "" }
        else if c.truthy("last_error") || (st.is_object() && !st.truthy("ok")) { "err" }
        else if st.g("signed_in") == &Value::Bool(false) { "warn" }
        else if st.truthy("ok") { "ok" } else { "" };
    format!(r##"<span class="dot {tone}" style="vertical-align:middle;margin-right:8px"></span>"##)
}

/// The status line of an installed plugin: what it is doing, whether it works, the last error.
pub fn status_html(c: &Value, acting: Option<&str>) -> String {
    let busy = acting.map(str::to_string).or_else(|| c.st("busy").map(str::to_string));
    if let Some(b) = busy { return format!(r##"<div class="muted">{SPIN} {}</div>"##, busy_words(&b)); }
    if !c.truthy("installed") { return String::new(); }
    let mut out = if !c.truthy("enabled") {
        r##"<div class="muted">Disabled – Encore does not use it until you enable it again</div>"##.to_string()
    } else {
        let st = c.g("status");
        if !st.is_object() {
            format!(r##"<div class="muted">{SPIN} Checking…</div>"##)
        } else if st.truthy("ok") {
            let m = if st.truthy("message") { st.s("message") } else { "Ready".into() };
            format!(r##"<div style="word-break:break-word">{}</div>"##, esc(&m))
        } else {
            let m = if st.truthy("message") { st.s("message") } else { "Not working".into() };
            format!(r##"<div class="row" style="gap:6px;align-items:flex-start;color:var(--err)"><span style="word-break:break-word">{}</span>{}</div>"##, esc(&m), crate::details::copy_button(&m))
        }
    };
    if c.truthy("last_error") {
        let e = c.s("last_error");
        out.push_str(&format!(r##"<div class="row" style="gap:6px;align-items:flex-start;color:var(--err);margin-top:4px"><span style="word-break:break-word">{}</span>{}</div>"##, esc(&e), crate::details::copy_button(&e)));
    }
    out
}

/// The buttons of a card (an installed plugin) or a row (one that is not). `acting`: what this page is doing
/// to it; `test_label`: the Test button's words.
pub fn actions_html(c: &Value, acting: Option<&str>, test_label: &str) -> String {
    let busy = acting.is_some() || c.truthy("busy");
    let dis = if busy { "disabled" } else { "" };
    let btn = |act: &str, cls: &str, ic: &str, words: String| -> String {
        if acting == Some(act) {
            format!(r##"<button class="btn sm {cls}" type="button" data-act="{act}" disabled>{SPIN} {}</button>"##, busy_words(act))
        } else {
            format!(r##"<button class="btn sm {cls}" type="button" data-act="{act}" {dis}>{}{}</button>"##, if ic.is_empty() { String::new() } else { icon(ic) }, esc(&words))
        }
    };
    if !c.truthy("installed") {
        return btn("install", "primary", "download", "Install".into());
    }
    let mut out = String::new();
    if c.truthy("can_update") {
        if c.truthy("update_available") && c.truthy("latest") { out.push_str(&btn("update", "primary", "download", format!("Update to {}", c.s("latest")))); }
        else { out.push_str(&btn("update", "", "download", "Update".into())); }
    }
    out.push_str(&btn("test", "", "refresh", test_label.to_string()));
    out.push_str(&btn("logs", "", "list", "Logs".into()));
    if c.truthy("enabled") { out.push_str(&btn("disable", "", "", "Disable".into())); } else { out.push_str(&btn("enable", "primary", "check", "Enable".into())); }
    out.push_str(&btn("remove", "danger", "trash", "Remove".into()));
    if c.truthy("can_update") {
        let words = if c.s("id") == "ytdlp" { "Update automatically (recommended – sites change often)" } else { "Update automatically when a new release is out" };
        out.push_str(&format!(r##"<label class="switch small" style="flex-basis:100%"><input type="checkbox" data-auto {} {dis}><i></i>{words}</label>"##,
            if c.truthy("auto_update") { "checked" } else { "" }));
    }
    out
}

/// The line about a plugin: what it finds, its sandbox and permissions (a third-party plugin), its homepage
/// and licence.
pub fn about_html(c: &Value) -> String {
    let mut parts: Vec<String> = vec![];
    if c.s("kind") == "source" {
        let finds = kinds_words(&c.a("kinds"));
        if !finds.is_empty() { parts.push(esc(&format!("Finds {finds}{}", if c.truthy("lossless") { ", lossless" } else { "" }))); }
    }
    if c.s("trust") == "third-party" {
        if c.truthy("sandbox") { parts.push(format!("Sandbox: {}", esc(&c.s("sandbox")))); }
        let hosts: Vec<String> = c.a("hosts").iter().map(js_string).filter(|h| !h.is_empty()).collect();
        parts.push(if !c.truthy("network") { "No network".into() } else if hosts.is_empty() { "Network: any address".into() } else { format!("Network: {}", esc(&hosts.join(", "))) });
        if c.truthy("reads_library") { parts.push("Reads your library (read only)".into()); }
    }
    let home = c.s("homepage");
    if safe_link(&home) { parts.push(format!(r##"<a href="{}" target="_blank" rel="noopener noreferrer" style="color:var(--accent)">Homepage</a>"##, esc(home.trim()))); }
    if c.truthy("license") { parts.push(format!("Licence {}", esc(&c.s("license")))); }
    if parts.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:6px">{}</div>"##, parts.join(" · ")) }
}

/// The logs drawer of a card (filled when Logs is pressed).
pub fn logs_html() -> String {
    format!(r##"<div class="card flat mt" data-logs hidden style="padding:12px 14px">
      <div class="row between wrap" style="gap:8px"><b>Log</b><span class="row" style="gap:6px"><button class="btn sm" type="button" data-logs-refresh>{refresh}Refresh</button><button class="btn sm" type="button" data-logs-copy>{copy}Copy</button></span></div>
      <div class="small faint mono" data-logs-file style="margin-top:4px;word-break:break-all"></div>
      <pre class="mono small" data-logs-text style="white-space:pre-wrap;word-break:break-word;max-height:320px;overflow:auto;margin:8px 0 0">Loading…</pre>
    </div>"##, refresh = icon("refresh"), copy = icon("copy"))
}

// ---- the settings of a card ---------------------------------------------------------------------------------

/// The fields of a settings answer (GET /api/plugins/{id}/settings) in runs under their `part` heading (a part
/// holds its field and the ones after it).
pub fn runs(answer: &Value) -> Vec<(String, Vec<Value>)> {
    let mut out: Vec<(String, Vec<Value>)> = vec![];
    let mut part = String::new();
    for f in answer.a("fields") {
        if f.truthy("part") { part = f.s("part"); }
        match out.last_mut() {
            Some((p, fs)) if *p == part => fs.push(f),
            _ => out.push((part.clone(), vec![f])),
        }
    }
    out
}

/// A field of a plugin card. A `when` rule naming a select of another plugin (a client's field names the
/// Torrents & Usenet card's choice) is left out: the client's card is shown whole.
pub fn field_of(f: &Value, keys: &[String]) -> Field {
    let mut field = Field::from_json(f);
    if field.when.as_ref().is_some_and(|w| !keys.contains(&w.key)) { field.when = None; }
    field
}

/// A plugin card's settings as a [`Group`] (for Save, and for the VPN block's quiet save): its id is the plugin's.
pub fn group_of(answer: &Value, title: &str) -> Group {
    let keys: Vec<String> = answer.a("fields").iter().map(|f| f.s("key")).collect();
    Group {
        id: answer.s("id"),
        title: title.to_string(),
        desc: String::new(),
        part: String::new(),
        test: None,
        test_label: None,
        fields: answer.a("fields").iter().map(|f| field_of(f, &keys)).collect(),
    }
}

/// The values a card's fields show, with `_meta` (where each comes from). A built-in plugin's keys are Encore's
/// settings: GET /admin/settings (`settings`) has them with their `_meta`; a key it lacks is taken from the
/// plugin's answer. A plugin's own settings are only in its answer (saved ones show as saved, none locked).
pub fn card_values(answer: &Value, settings: &Value) -> Value {
    let builtin = answer.truthy("builtin");
    let mut values = if builtin && settings.is_object() { settings.as_object().cloned().unwrap_or_default() } else { Map::new() };
    let mut meta = values.get("_meta").and_then(|m| m.as_object().cloned()).unwrap_or_default();
    let given = answer.o("values");
    for f in answer.a("fields") {
        let key = f.s("key");
        if !values.contains_key(&key) { values.insert(key.clone(), given.g(&key).clone()); }
        if !meta.contains_key(&key) {
            let source = if f.truthy("env_locked") { "env" } else if builtin { "default" } else { "ui" };
            meta.insert(key, json!({ "source": source, "env": f.g("env") }));
        }
    }
    values.insert("_meta".into(), Value::Object(meta));
    Value::Object(values)
}

/// What a part heading says, from the settings schema: a part that was a card of its own before 0.32 (YouTube
/// sign-in) keeps that card's description and Test button.
pub struct PartInfo {
    pub desc: String,
    /// (test, its words) for POST /api/system/test/<test>
    pub test: Option<(String, String)>,
    /// the schema card it was
    pub card: String,
}

pub fn part_info(schema: &Schema, part: &str) -> PartInfo {
    let g = schema.sections.iter().flat_map(|s| s.groups.iter()).find(|g| g.title == part);
    PartInfo {
        desc: g.map(|g| g.desc.clone()).unwrap_or_default(),
        test: g.and_then(|g| g.test.clone().map(|t| (t, g.test_label.clone().unwrap_or_else(|| "Test".into())))),
        card: g.map(|g| g.id.clone()).unwrap_or_default(),
    }
}

/// The form of a plugin card: its fields run by run (schema_form.rs), a part with its heading, description and
/// Test, and the YouTube sign-in block in the part that was the YouTube sign-in card. `reveal` (the eye and
/// copy buttons of saved secrets) applies to a built-in plugin's keys only.
pub fn form_body_html(answer: &Value, values: &Value, reveal: bool, schema: &Schema) -> String {
    // the eye and copy buttons ask GET /admin/settings/secret/<key>: Encore's own keys only
    let reveal = reveal && answer.truthy("builtin");
    let keys: Vec<String> = answer.a("fields").iter().map(|f| f.s("key")).collect();
    let mut html = String::new();
    for (part, fields) in runs(answer) {
        let fields: Vec<Field> = fields.iter().map(|f| field_of(f, &keys)).collect();
        let refs: Vec<&Field> = fields.iter().collect();
        if !part.is_empty() {
            let info = part_info(schema, &part);
            let test = info.test.as_ref().map(|(t, words)| format!(r##"<button class="btn sm" type="button" data-part-test="{}">{}{}</button>"##, esc(t), icon("refresh"), esc(words))).unwrap_or_default();
            html.push_str(&format!(r##"<div data-part="{p}" style="margin-top:22px"><div class="row between" style="gap:10px;align-items:flex-start"><div style="min-width:0"><h3>{p}</h3>{desc}</div>{test}</div><div data-part-out style="margin-top:8px"></div></div>"##,
                p = esc(&part),
                desc = if info.desc.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:4px">{}</div>"##, esc(&info.desc)) }));
            if info.card == "youtube" { html.push_str(&crate::pages::youtube::youtube_block()); }
        }
        html.push_str(&form_html(&refs, values, reveal));
    }
    html
}

/// The blocks a plugin card draws under its fields: Tidal sign-in and the streamrip config (streamrip), the
/// Soulseek status (slskd).
pub fn extras_html(id: &str) -> String {
    match id {
        "streamrip" => apps::tidal_block() + &apps::streamrip_config_block(),
        "slskd" => format!("\n      {}", apps::soulseek_status_block()),
        _ => String::new(),
    }
}

/// A download client's card: which choice of the Torrents & Usenet card sends downloads to it (`rule`: its
/// fields' `when`, `select_label`/`option_label`: that select's words), and whether it is chosen now.
pub fn client_note_html(name: &str, select_label: &str, option_label: &str, chosen: bool, chosen_label: &str) -> String {
    if chosen {
        format!(r##"<div class="small muted" style="margin-bottom:10px">In use: {} is set to {} on the Torrents &amp; Usenet card.</div>"##, esc(select_label), esc(option_label))
    } else {
        format!(r##"<div class="small" style="color:var(--warn);margin-bottom:10px">Not in use: {} is set to {} on the Torrents &amp; Usenet card. Choose {} there to send downloads to {}.</div>"##,
            esc(select_label), esc(if chosen_label.is_empty() { "something else" } else { chosen_label }), esc(option_label), esc(name))
    }
}

/// The Torrents & Usenet card: each chosen client has a plugin of its own, which has to be installed and on.
/// `clients`: (select's words, chosen value's words, the client plugin's card) for the two selects.
pub fn chosen_clients_html(clients: &[(String, String, Value)]) -> String {
    let mut out = String::new();
    for (select, chosen, c) in clients {
        if !c.is_object() { continue; }
        let line = if !c.truthy("installed") {
            format!(r##"{}: {} has a plugin of its own, not installed yet. <button class="btn sm" type="button" data-act="install" data-id="{}">{}Install</button>"##,
                esc(select), esc(chosen), esc(&c.s("id")), icon("download"))
        } else if !c.truthy("enabled") {
            format!(r##"{}: the {} plugin is disabled. <button class="btn sm" type="button" data-act="enable" data-id="{}">Enable</button>"##, esc(select), esc(chosen), esc(&c.s("id")))
        } else { continue };
        out.push_str(&format!(r##"<div class="small row wrap" style="gap:8px;color:var(--warn);margin-top:8px">{line}</div>"##));
    }
    out
}

// ---- cards and the page -------------------------------------------------------------------------------------

/// An installed plugin's card. `body`: its settings (or the line saying where they are), `open`: whether it
/// starts open.
pub fn installed_card_html(c: &Value, body: &str, open: bool, acting: Option<&str>, test_label: &str) -> String {
    let chev = { let c = icon("chevron"); if c.is_empty() { "▾".to_string() } else { c } };
    let warning = if c.truthy("warning") {
        format!(r##"<div class="small" style="color:var(--warn);margin-bottom:10px;display:flex;gap:8px;align-items:flex-start">{}<span>{}</span></div>"##, crate::ui::icon_attr("alert", r#"width="16" height="16" style="flex:none;margin-top:1px""#), esc(&c.s("warning")))
    } else { String::new() };
    format!(r##"<details class="card settings-card" data-plugin="{id}" {open}>
      <summary class="row between" style="gap:10px;flex-wrap:nowrap;align-items:flex-start"><div style="min-width:0">
        <h2><span data-dot>{dot}</span>{name} <span class="small muted mono" data-ver>{ver}</span></h2>
        <div class="row wrap" style="gap:6px;margin-top:6px" data-badges>{badges}</div>
        <div class="small muted" style="margin-top:6px;font-weight:400">{desc}</div></div>
        <span class="chev">{chev}</span></summary>
      <div style="height:12px"></div>
      {warning}<div class="small" data-status>{status}</div>
      {about}
      <div class="row wrap mt" style="gap:8px;align-items:center" data-actions>{actions}</div>
      <div class="mt" data-out></div>
      {logs}
      {body}
    </details>"##,
        id = esc(&c.s("id")), open = if open { "open" } else { "" }, dot = dot_html(c), name = esc(&c.s("name")), ver = esc(&version_text(c)),
        badges = badges_html(c), desc = esc(&c.s("description")), status = status_html(c, acting), about = about_html(c),
        actions = actions_html(c, acting, test_label), logs = logs_html())
}

/// A plugin that is not installed: a row of the Available list.
pub fn available_html(c: &Value, acting: Option<&str>) -> String {
    format!(r##"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-plugin-row="{id}">
      <div class="grow" style="min-width:200px"><div style="font-weight:650">{name} <span class="small muted mono">{ver}</span></div>
        <div class="row wrap" style="gap:6px;margin-top:4px" data-badges>{badges}</div>
        <div class="small muted" style="margin-top:4px">{desc}</div>{about}<div class="small" data-status>{status}</div></div>
      <div class="row" style="gap:8px;align-items:center" data-actions>{actions}</div></div>"##,
        id = esc(&c.s("id")), name = esc(&c.s("name")), ver = esc(&c.s("version")), badges = badges_html(c), desc = esc(&c.s("description")),
        about = about_html(c), status = status_html(c, acting), actions = actions_html(c, acting, "Test"))
}

/// The page's first card: Check for updates and the sandbox line.
pub fn head_html(list: &Value) -> String {
    format!(r##"<div class="card" id="plugins-head">
      <div class="row between wrap" style="gap:10px"><h2>Plugins</h2><button class="btn sm" type="button" id="plugins-check">{refresh}Check for updates</button></div>
      <p class="muted small" style="margin:8px 0 12px">Download sources and clients, server tools and connections to other music servers. Each one installs, updates and is removed on its own and is kept in the config folder, so no new Encore image is needed. Installed plugins are checked for updates daily; change the schedule under <a href="#/settings?s=tasks">Scheduled tasks</a>.</p>
      {sandbox}
    </div>"##, refresh = icon("refresh"), sandbox = sandbox_html(list.g("sandbox")))
}

/// Upload plugin and Install from URL or GitHub, with the third-party warning.
pub fn add_html(list: &Value) -> String {
    let max = if list.i("upload_max_mb") > 0 { list.i("upload_max_mb") } else { 200 };
    format!(r##"<div class="card mt" id="plugins-add">
      <h3>Add a plugin</h3>
      <p class="small muted" style="margin:6px 0 12px">A plugin is a zip with a plugin.toml and its program, for example one made with Encore's plugin SDK (docs/plugins in Encore's repository). Plugins you add are not made by Encore: install only plugins you trust.</p>
      <div class="row wrap" style="gap:8px;align-items:center">
        <input type="file" id="plugin-file" accept=".zip,application/zip" hidden>
        <button class="btn" type="button" id="plugin-upload">{plus}Upload plugin</button>
        <span class="small muted">A .zip of at most {max} MB</span>
      </div>
      <form class="row wrap mt" style="gap:8px" id="plugin-url-form">
        <input class="input" style="flex:1 1 260px;min-width:0" id="plugin-url" placeholder="https://… or owner/repo@ref" autocomplete="off" spellcheck="false" aria-label="Plugin address">
        <button class="btn" type="submit">{link}Install from URL</button>
      </form>
      <div class="small muted" style="margin-top:6px">The address of a plugin zip or of its plugin.toml, or a GitHub repository as owner/repo@ref (a branch, tag or commit; owner/repo/folder@ref for a plugin in a folder).</div>
      <div class="mt" id="plugin-add-out"></div>
    </div>"##, plus = icon("plus"), link = icon("link"))
}

/// The whole page: the head, the installed plugins as cards (`body` gives a card's settings, `open` the cards
/// that start open), the Available list and Add a plugin.
pub fn page_html(list: &Value, body: impl Fn(&Value) -> String, open: &[String], acting: &HashMap<String, String>, test_label: impl Fn(&str) -> String) -> String {
    let plugins = list.a("plugins");
    let installed: Vec<&Value> = plugins.iter().filter(|c| c.truthy("installed")).collect();
    let available: Vec<&Value> = plugins.iter().filter(|c| !c.truthy("installed")).collect();
    let mut html = format!(r##"<div id="plugins-page">{}"##, head_html(list));
    html.push_str(r##"<div class="nav-label" style="padding:18px 4px 8px">Installed</div>"##);
    if installed.is_empty() {
        html.push_str(r##"<div class="card"><p class="muted small" style="margin:0">No plugins installed yet. Install the ones you want to use from the list below.</p></div>"##);
    }
    for c in &installed {
        let id = c.s("id");
        html.push_str(&installed_card_html(c, &body(c), open.contains(&id), acting.get(&id).map(String::as_str), &test_label(&id)));
    }
    if !available.is_empty() {
        html.push_str(r##"<div class="nav-label" style="padding:18px 4px 8px">Available</div><div class="card flat"><div class="list" id="plugins-available">"##);
        for c in &available { html.push_str(&available_html(c, acting.get(&c.s("id")).map(String::as_str))); }
        html.push_str("</div></div>");
    }
    html.push_str(&add_html(list));
    html.push_str("</div>");
    html
}

/// The settings part of an installed card: the form with Save, the line naming the page that has them, or why
/// they could not be read.
pub fn body_html(c: &Value, answer: Option<&Value>, settings: &Value, reveal: bool, schema: &Schema, list: &Value) -> String {
    let on = c.s("settings_on");
    if !on.is_empty() {
        let title = schema.section(&on).map(|s| s.title.clone()).unwrap_or_else(|| on.clone());
        return format!(r##"<div class="small muted mt">Its settings are on <a href="#/settings?s={}" style="color:var(--accent)">Settings → {}</a>.</div>"##, esc(&on), esc(&title));
    }
    if !c.truthy("has_settings") { return String::new(); }
    let Some(answer) = answer else { return String::new() };
    if answer.truthy("_error") { return format!(r##"<div class="mt">{}</div>"##, error_html(&answer.s("_error"))); }
    let values = card_values(answer, settings);
    let mut html = String::from(r##"<div class="mt" data-form style="border-top:1px solid var(--border);padding-top:16px">"##);
    if c.s("kind") == "client" { html.push_str(&format!("<div data-client-note>{}</div>", client_note(c, answer, settings, schema))); }
    html.push_str(&form_body_html(answer, &values, reveal, schema));
    html.push_str(&extras_html(&c.s("id")));
    if c.s("id") == "torznab" { html.push_str(&format!("<div data-clients>{}</div>", chosen_clients_html(&chosen_clients(settings, list, schema)))); }
    html.push_str(&format!(r##"<div class="row mt" style="justify-content:flex-end"><button class="btn primary" type="button" data-save>{}Save</button></div></div>"##, icon("check")));
    html
}

/// A field of the settings schema by key.
fn schema_field<'a>(schema: &'a Schema, key: &str) -> Option<&'a Field> {
    schema.sections.iter().flat_map(|s| s.groups.iter()).flat_map(|g| g.fields.iter()).find(|f| f.key == key)
}

/// A client card's note (see [`client_note_html`]), from its fields' `when` rule.
fn client_note(c: &Value, answer: &Value, settings: &Value, schema: &Schema) -> String {
    let Some(rule) = answer.a("fields").iter().filter_map(|f| f.st("when").and_then(super::fields::When::parse)).find(|w| w.equal) else { return String::new() };
    let select = schema_field(schema, &rule.key);
    let words = |v: &str| select.and_then(|f| f.options.iter().find(|(o, _)| o == v).map(|(_, l)| l.clone())).unwrap_or_else(|| v.to_string());
    let chosen = settings.s(&rule.key);
    client_note_html(&c.s("name"), &select.map(|f| f.label.clone()).unwrap_or_else(|| rule.key.clone()), &words(&rule.value), chosen == rule.value, &words(&chosen))
}

/// The two clients the Torrents & Usenet card has chosen, with their plugins' cards.
fn chosen_clients(settings: &Value, list: &Value, schema: &Schema) -> Vec<(String, String, Value)> {
    let plugins = list.a("plugins");
    ["torrent_client", "nzb_client"].iter().filter_map(|key| {
        let v = settings.s(key);
        let (_, _, id) = CLIENTS.iter().find(|(k, val, _)| k == key && *val == v)?;
        let c = plugins.iter().find(|c| c.s("id") == *id)?.clone();
        let f = schema_field(schema, key);
        let label = f.map(|f| f.label.clone()).unwrap_or_else(|| key.to_string());
        let chosen = f.and_then(|f| f.options.iter().find(|(o, _)| *o == v).map(|(_, l)| l.clone())).unwrap_or(v);
        Some((label, chosen, c))
    }).collect()
}

// ---- the page in the browser --------------------------------------------------------------------------------

thread_local! {
    /// Bumped on every drawing of the page, so the work of an earlier one (status calls, the busy poll) stops.
    static GEN: Cell<u32> = const { Cell::new(0) };
}

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

struct Page {
    hub: Rc<Hub>,
    root: Element,
    gen: u32,
    list: RefCell<Value>,
    /// the settings answers of the cards drawn with a form
    forms: RefCell<HashMap<String, Value>>,
    groups: RefCell<HashMap<String, Rc<Group>>>,
    acting: RefCell<HashMap<String, String>>,
}

/// Settings → Plugins: draws the page, the card a `?s=` link named open.
pub async fn show_page(hub: &Rc<Hub>) {
    let focus = hub.focus.borrow_mut().take();
    let open: Vec<String> = focus.iter().map(|f| card_for(f).to_string()).collect();
    draw(hub, open, focus).await;
}

/// The cards open now: they stay open when the page is drawn again.
fn open_cards(hub: &Hub) -> Vec<String> {
    dom::qsa_in(&hub.panel, "details[data-plugin][open]").iter().map(|d| dom::data(d, "plugin")).collect()
}

async fn redraw(hub: &Rc<Hub>, also_open: Option<&str>, focus: Option<String>) {
    let mut open = open_cards(hub);
    if let Some(id) = also_open { open.push(id.to_string()); }
    draw(hub, open, focus).await;
}

fn current(gen: u32) -> bool { GEN.with(|g| g.get()) == gen }

/// Whether Settings still shows the Plugins page (another page may have been picked while an answer was on its
/// way: it must not be drawn over).
fn showing(hub: &Hub) -> bool {
    dom::qs_in(&hub.view, ".settings-nav button.active").is_some_and(|b| dom::data(&b, "s") == "plugins")
}

async fn draw(hub: &Rc<Hub>, open: Vec<String>, focus: Option<String>) {
    let gen = GEN.with(|g| { g.set(g.get().wrapping_add(1)); g.get() });
    if dom::qs_in(&hub.panel, "#plugins-page").is_none() {
        dom::set_html(&hub.panel, r##"<div class="skeleton" style="height:200px"></div>"##);
    }
    let list = match api::get("/plugins").await {
        Ok(l) => l,
        Err(e) => {
            if !current(gen) || !showing(hub) { return; }
            dom::set_html(&hub.panel, &format!(r##"<div class="card" id="plugins-page"><h2>Plugins</h2><div class="mt">{}</div><button class="btn sm" type="button" data-retry>{}Try again</button></div>"##,
                error_html(&error_text(&e.message, &e.data)), icon("refresh")));
            if let Some(b) = dom::qs_in(&hub.panel, "[data-retry]") {
                let hub = hub.clone();
                dom::on(&b, "click", move |_| { let hub = hub.clone(); dom::spawn(async move { draw(&hub, vec![], None).await }); });
            }
            return;
        }
    };
    // the settings of the installed plugins that have a card of their own
    let ids: Vec<String> = list.a("plugins").iter()
        .filter(|c| c.truthy("installed") && c.truthy("has_settings") && c.s("settings_on").is_empty())
        .map(|c| c.s("id")).collect();
    let answers = futures::future::join_all(ids.iter().map(|id| {
        let path = format!("/plugins/{}/settings", enc(id));
        async move { api::get(&path).await }
    })).await;
    if !current(gen) || !showing(hub) { return; }
    let forms: HashMap<String, Value> = ids.iter().cloned().zip(answers.into_iter().map(|r| match r {
        Ok(v) => v,
        Err(e) => json!({ "_error": error_text(&e.message, &e.data) }),
    })).collect();
    let settings = hub.values.borrow().clone();
    let schema = hub.schema.clone();
    let acting: HashMap<String, String> = HashMap::new();
    let html = page_html(&list, |c| body_html(c, forms.get(&c.s("id")), &settings, hub.reveal, &schema, &list), &open, &acting, |id| test_label(&schema, id));
    dom::set_html(&hub.panel, &html);
    let Some(root) = dom::qs_in(&hub.panel, "#plugins-page") else { return };
    let page = Rc::new(Page {
        hub: hub.clone(), root, gen, list: RefCell::new(list), forms: RefCell::new(forms),
        groups: RefCell::new(HashMap::new()), acting: RefCell::new(HashMap::new()),
    });
    page.bind();
    if let Some(f) = focus {
        let card = card_for(&f).to_string();
        let el = dom::qs_in(&page.root, &format!(r#"[data-plugin="{card}"]"#)).or_else(|| dom::qs_in(&page.root, &format!(r#"[data-plugin-row="{card}"]"#)));
        let part = if f == "youtube" { el.as_ref().and_then(|c| dom::qs_in(c, r#"[data-part="YouTube sign-in"]"#)) } else { None };
        if let Some(e) = part.or(el) { e.scroll_into_view(); }
    }
    dom::spawn(page.clone().statuses());
    if page.list.borrow().a("plugins").iter().any(|c| c.truthy("busy")) { dom::spawn(page.clone().poll_busy()); }
}

/// The Test button's words: those of the card the plugin's settings were before 0.32, or "Test".
fn test_label(schema: &Schema, id: &str) -> String {
    schema.group(id).and_then(|g| g.test.as_ref().and(g.test_label.clone())).unwrap_or_else(|| "Test".into())
}

impl Page {
    fn alive(&self) -> bool { current(self.gen) && dom::is_connected(&self.root) }

    fn card(&self, id: &str) -> Value {
        self.list.borrow().a("plugins").into_iter().find(|c| c.s("id") == id).unwrap_or_else(|| json!({ "id": id }))
    }

    fn set_card(&self, c: &Value) {
        if !c.is_object() || !c.truthy("id") { return; }
        let mut list = self.list.borrow_mut();
        if let Some(a) = list.get_mut("plugins").and_then(|p| p.as_array_mut()) {
            if let Some(slot) = a.iter_mut().find(|x| x.s("id") == c.s("id")) { *slot = c.clone(); }
        }
    }

    fn el(&self, id: &str) -> Option<Element> {
        dom::qs_in(&self.root, &format!(r#"[data-plugin="{id}"]"#)).or_else(|| dom::qs_in(&self.root, &format!(r#"[data-plugin-row="{id}"]"#)))
    }

    /// Draws a card's (or row's) badges, status and buttons again from what the page knows.
    fn paint(&self, id: &str) {
        let Some(el) = self.el(id) else { return };
        let c = self.card(id);
        let acting = self.acting.borrow().get(id).cloned();
        let part = |sel: &str, html: String| { if let Some(x) = dom::qs_in(&el, sel) { dom::set_html(&x, &html); } };
        part("[data-badges]", badges_html(&c));
        part("[data-status]", status_html(&c, acting.as_deref()));
        part("[data-actions]", actions_html(&c, acting.as_deref(), &test_label(&self.hub.schema, id)));
        part("[data-dot]", dot_html(&c));
        if let Some(v) = dom::qs_in(&el, "[data-ver]") { dom::set_text(&v, &version_text(&c)); }
    }

    /// The lines that depend on the clients the Torrents & Usenet card chose (after it is saved, or a client
    /// plugin is installed): its own hint and each client card's note.
    fn paint_clients(&self) {
        let (settings, list, schema) = (self.hub.values.borrow().clone(), self.list.borrow().clone(), self.hub.schema.clone());
        if let Some(x) = self.el("torznab").and_then(|c| dom::qs_in(&c, "[data-clients]")) {
            dom::set_html(&x, &chosen_clients_html(&chosen_clients(&settings, &list, &schema)));
        }
        for (id, answer) in self.forms.borrow().iter() {
            let c = self.card(id);
            if c.s("kind") != "client" { continue; }
            if let Some(x) = self.el(id).and_then(|e| dom::qs_in(&e, "[data-client-note]")) { dom::set_html(&x, &client_note(&c, answer, &settings, &schema)); }
        }
    }

    fn paint_all(&self) {
        for c in self.list.borrow().clone().a("plugins") { self.paint(&c.s("id")); }
    }

    /// Whether the plugins installed differ from those drawn (an install or remove elsewhere): then the page is
    /// drawn again.
    fn changed_shape(&self, list: &Value) -> bool {
        list.a("plugins").iter().any(|c| c.truthy("installed") != dom::qs_in(&self.root, &format!(r#"[data-plugin="{}"]"#, c.s("id"))).is_some())
    }

    fn out(&self, id: &str) -> Option<Element> { self.el(id).and_then(|e| dom::qs_in(&e, "[data-out]")) }

    fn bind(self: &Rc<Self>) {
        // each card with a form: its group, the `when` rules, and the extras of the built-in cards
        for card in dom::qsa_in(&self.root, "details[data-plugin]") {
            let id = dom::data(&card, "plugin");
            let Some(answer) = self.forms.borrow().get(&id).cloned() else { continue };
            if answer.truthy("_error") { continue; }
            let g = Rc::new(group_of(&answer, &self.card(&id).s("name")));
            self.groups.borrow_mut().insert(id.clone(), g.clone());
            bind_show_when(&card);
            if !answer.truthy("builtin") { continue; }
            if self.hub.reveal { groups::bind_secret_buttons(&card); }
            match id.as_str() {
                "streamrip" => { apps::bind_tidal(&self.hub.view, &card); apps::bind_streamrip_config(&card); }
                "slskd" => { apps::bind_vpn(&self.hub, g.clone(), &card); apps::bind_soulseek(&self.hub, &card); }
                "ytdlp" => crate::pages::youtube::bind_youtube(&card),
                _ => {}
            }
        }
        let me = self.clone();
        dom::on(&self.root, "click", move |e| me.clicked(e));
        let me = self.clone();
        dom::on(&self.root, "change", move |e| me.changed(e));
        if let Some(form) = dom::qs_in(&self.root, "#plugin-url-form") {
            let me = self.clone();
            dom::on(&form, "submit", move |e| {
                dom::prevent(e);
                let input = dom::value_of(&me.root, "#plugin-url");
                dom::spawn(me.clone().install_url(input));
            });
        }
    }

    /// The id of the plugin a button belongs to: its own `data-id`, else its card's or row's.
    fn id_of(b: &Element) -> String {
        dom::data_opt(b, "id").filter(|s| !s.is_empty())
            .or_else(|| dom::closest_of(b, "[data-plugin]").map(|c| dom::data(&c, "plugin")))
            .or_else(|| dom::closest_of(b, "[data-plugin-row]").map(|c| dom::data(&c, "pluginRow")))
            .unwrap_or_default()
    }

    fn clicked(self: &Rc<Self>, e: &web_sys::Event) {
        if let Some(b) = dom::closest(e, "[data-act]") {
            dom::prevent(e);
            if b.has_attribute("disabled") { return; }
            dom::spawn(self.clone().act(Self::id_of(&b), dom::data(&b, "act")));
        } else if let Some(b) = dom::closest(e, "[data-save]") {
            let id = Self::id_of(&b);
            let me = self.clone();
            dom::spawn(async move { let _ = me.save(&id, false).await; });
        } else if let Some(b) = dom::closest(e, "[data-part-test]") {
            dom::prevent(e);
            dom::spawn(self.clone().part_test(Self::id_of(&b), b));
        } else if let Some(a) = dom::closest(e, "[data-clear]") {
            dom::prevent(e);
            dom::spawn(self.clone().clear(Self::id_of(&a), dom::data(&a, "clear")));
        } else if let Some(b) = dom::closest(e, "[data-logs-refresh]") {
            dom::spawn(self.clone().logs(Self::id_of(&b), false));
        } else if let Some(b) = dom::closest(e, "[data-logs-copy]") {
            if let Some(pre) = dom::closest_of(&b, "[data-logs]").and_then(|d| dom::qs_in(&d, "[data-logs-text]")) { copy_text(&dom::text_of(&pre)); }
        } else if dom::closest(e, "#plugins-check").is_some() {
            dom::spawn(self.clone().check_all());
        } else if dom::closest(e, "#plugin-upload").is_some() {
            if let Some(input) = dom::qs_in(&self.root, "#plugin-file") { dom::click(&input); }
        }
    }

    fn changed(self: &Rc<Self>, e: &web_sys::Event) {
        let Some(t) = dom::target_el(e) else { return };
        if t.has_attribute("data-auto") {
            dom::spawn(self.clone().auto_update(Self::id_of(&t), t));
        } else if t.id() == "plugin-file" {
            let Some(input) = t.dyn_ref::<HtmlInputElement>() else { return };
            let file = input.files().and_then(|l| l.get(0));
            input.set_value("");
            if let Some(f) = file { dom::spawn(self.clone().upload(f)); }
        }
    }

    /// Install, Update, Enable, Disable, Remove, Test and Logs.
    async fn act(self: Rc<Self>, id: String, act: String) {
        if id.is_empty() { return; }
        let before = self.card(&id);
        let name = if before.truthy("name") { before.s("name") } else { id.clone() };
        match act.as_str() {
            "logs" => return self.logs(id, true).await,
            "test" => return self.test(id).await,
            "remove" => {
                let text = if before.s("origin") == "catalog" {
                    format!("Remove {name}? Encore stops using it until you install it again. What the plugin downloaded is deleted; your settings for it are kept.")
                } else {
                    format!("Remove {name}? Its program, data, log and settings are deleted.")
                };
                if !crate::ui::confirm_dialog(&format!("Remove {name}"), &text, "Remove", true).await { return; }
            }
            "install" | "update" | "enable" | "disable" => {}
            _ => return,
        }
        self.acting.borrow_mut().insert(id.clone(), act.clone());
        self.paint(&id);
        if let Some(out) = self.out(&id) { dom::set_html(&out, ""); }
        let r = api::post_empty(&format!("/plugins/{}/{act}", enc(&id))).await;
        self.acting.borrow_mut().remove(&id);
        if !self.alive() { return; }
        match r {
            Ok(c) => {
                toast(&done_words(&act, &before, &c), "ok");
                if act == "install" {
                    // the new card, open, where it now is (Installed, above the list it came from)
                    redraw(&self.hub, Some(id.as_str()), Some(id.clone())).await;
                    return;
                }
                if act == "remove" { redraw(&self.hub, None, None).await; return; }
                self.set_card(&c);
                self.paint(&id);
                if act != "disable" { self.clone().status_of(id).await; }
            }
            Err(e) => {
                let text = error_text(&e.message, &e.data);
                match self.out(&id) {
                    Some(out) => dom::set_html(&out, &error_html(&text)),
                    None => toast(&text, "err"),
                }
                // the card as the server has it now (its last error)
                if let Ok(c) = api::get(&format!("/plugins/{}", enc(&id))).await { self.set_card(&c); }
                self.paint(&id);
            }
        }
    }

    /// Test: the card's settings saved quietly first (as the cards always did), then POST /plugins/{id}/test.
    async fn test(self: Rc<Self>, id: String) {
        let Some(out) = self.out(&id) else { return };
        self.acting.borrow_mut().insert(id.clone(), "test".into());
        self.paint(&id);
        dom::set_html(&out, r##"<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>"##);
        let saved = if self.groups.borrow().contains_key(&id) { self.save(&id, true).await } else { Ok(()) };
        let html = match saved {
            Err(m) => test_html(&json!({ "ok": false, "error": m })),
            Ok(()) => match api::post_empty(&format!("/plugins/{}/test", enc(&id))).await {
                Ok(r) => test_html(&r),
                Err(e) => error_html(&error_text(&e.message, &e.data)),
            },
        };
        self.acting.borrow_mut().remove(&id);
        if !self.alive() { return; }
        dom::set_html(&out, &html);
        self.paint(&id);
        self.status_of(id).await;
    }

    /// Save a card: a built-in plugin's settings with PUT /admin/settings (Encore's keys), a plugin's own with
    /// PUT /plugins/{id}/settings. Err: the words of the failure (already toasted).
    async fn save(self: &Rc<Self>, id: &str, quiet: bool) -> Result<(), String> {
        let (Some(g), Some(answer), Some(card)) = (self.groups.borrow().get(id).cloned(), self.forms.borrow().get(id).cloned(), self.el(id)) else { return Ok(()) };
        if answer.truthy("builtin") {
            let model = self.hub.values.borrow().s("translator_model");
            self.hub.save(&g, true, &card).await.map_err(|e| e.message)?;
            if id == "torznab" { self.paint_clients(); }
            if !quiet {
                let changed_model = id == "translator" && self.hub.values.borrow().s("translator_model") != model;
                toast(if changed_model { "Saved – Install or Update the Translator to download this model" } else { "Settings saved" }, "ok");
            }
            return Ok(());
        }
        let meta = card_values(&answer, &Value::Null).o("_meta");
        let body = form_values(&g, &card, &meta);
        match api::put(&format!("/plugins/{}/settings", enc(id)), json!({ "values": Value::Object(body) })).await {
            Ok(a) => {
                self.forms.borrow_mut().insert(id.to_string(), a);
                if !quiet { toast("Settings saved", "ok"); }
                Ok(())
            }
            Err(e) => {
                let text = error_text(&e.message, &e.data);
                toast(&text, "err");
                Err(text)
            }
        }
    }

    /// A part's Test (YouTube sign-in's Test cookies): the card saved quietly, then POST /system/test/<test>.
    async fn part_test(self: Rc<Self>, id: String, b: Element) {
        let test = dom::data(&b, "partTest");
        let Some(out) = dom::closest_of(&b, "[data-part]").and_then(|p| dom::qs_in(&p, "[data-part-out]")) else { return };
        dom::set_disabled(&b, true);
        dom::set_html(&out, r##"<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>"##);
        let failed = |e: ApiError| json!({ "ok": false, "message": if e.message.is_empty() { "Test failed".to_string() } else { e.message } });
        let r = match self.save(&id, true).await {
            Ok(()) => match api::post_empty(&format!("/system/test/{test}")).await { Ok(r) => r, Err(e) => failed(e) },
            Err(m) => json!({ "ok": false, "message": m }),
        };
        dom::set_html(&out, &test_result(&r));
        dom::set_disabled(&b, false);
    }

    /// A saved secret's clear link.
    async fn clear(self: Rc<Self>, id: String, key: String) {
        let builtin = self.forms.borrow().get(&id).is_some_and(|a| a.truthy("builtin"));
        let mut body = Map::new();
        body.insert(key, Value::String(String::new()));
        let r = if builtin {
            api::put("/admin/settings", Value::Object(body)).await.map(|v| { *self.hub.values.borrow_mut() = v; })
        } else {
            api::put(&format!("/plugins/{}/settings", enc(&id)), json!({ "values": Value::Object(body) })).await.map(|_| ())
        };
        match r {
            Ok(()) => { toast("Cleared", "ok"); redraw(&self.hub, Some(&id), None).await; }
            Err(e) => toast(&e.message, "err"),
        }
    }

    /// The auto-update switch.
    async fn auto_update(self: Rc<Self>, id: String, input: Element) {
        let on = dom::checked(&input);
        match api::post(&format!("/plugins/{}/auto-update", enc(&id)), json!({ "on": on })).await {
            Ok(c) => { self.set_card(&c); toast("Saved", "ok"); }
            Err(e) => { dom::set_checked(&input, !on); toast(&e.message, "err"); }
        }
    }

    /// The logs drawer: `toggle` opens or closes it (and reads the log when it opens), else it is read again.
    async fn logs(self: Rc<Self>, id: String, toggle: bool) {
        let Some(drawer) = self.el(&id).and_then(|c| dom::qs_in(&c, "[data-logs]")) else { return };
        if toggle && !drawer.has_attribute("hidden") { dom::hide(&drawer, true); return; }
        dom::hide(&drawer, false);
        let (Some(text), Some(file)) = (dom::qs_in(&drawer, "[data-logs-text]"), dom::qs_in(&drawer, "[data-logs-file]")) else { return };
        dom::set_text(&text, "Loading…");
        match api::get(&format!("/plugins/{}/logs?lines=200", enc(&id))).await {
            Ok(r) => {
                let lines: Vec<String> = r.a("lines").iter().map(js_string).collect();
                dom::set_text(&text, &if lines.is_empty() { "The log is empty.".to_string() } else { lines.join("\n") });
                dom::set_text(&file, &r.s("file"));
                if let Some(h) = text.dyn_ref::<web_sys::HtmlElement>() { h.set_scroll_top(h.scroll_height()); }
            }
            Err(e) => dom::set_text(&text, &error_text(&e.message, &e.data)),
        }
    }

    /// One plugin's card with its status asked now (GET /plugins/{id}).
    async fn status_of(self: Rc<Self>, id: String) {
        if let Ok(c) = api::get(&format!("/plugins/{}", enc(&id))).await {
            if !self.alive() { return; }
            self.set_card(&c);
            self.paint(&id);
        }
    }

    /// The status of every enabled plugin, each painted as it comes.
    async fn statuses(self: Rc<Self>) {
        let ids: Vec<String> = self.list.borrow().a("plugins").iter().filter(|c| c.truthy("installed") && c.truthy("enabled")).map(|c| c.s("id")).collect();
        for id in ids { dom::spawn(self.clone().status_of(id)); }
    }

    /// While a plugin is being installed, updated or removed elsewhere (another tab, the scheduled task), the
    /// list is asked again every 3 s; once that is done the page shows the result.
    async fn poll_busy(self: Rc<Self>) {
        loop {
            dom::sleep(3000).await;
            if !self.alive() { return; }
            let Ok(list) = api::get("/plugins").await else { continue };
            if !self.alive() { return; }
            if self.changed_shape(&list) { redraw(&self.hub, None, None).await; return; }
            let busy = list.a("plugins").iter().any(|c| c.truthy("busy"));
            for c in list.a("plugins") { if !self.acting.borrow().contains_key(&c.s("id")) { self.set_card(&c); } }
            self.paint_all();
            if !busy { return; }
        }
    }

    /// Check for updates: every installed plugin that has a place to update from.
    async fn check_all(self: Rc<Self>) {
        let Some(b) = dom::qs_in(&self.root, "#plugins-check") else { return };
        dom::set_disabled(&b, true);
        dom::set_html(&b, &format!("{SPIN} Checking…"));
        let r = api::post_empty("/plugins/check").await;
        if !self.alive() { return; }
        dom::set_disabled(&b, false);
        dom::set_html(&b, &format!("{}Check for updates", icon("refresh")));
        match r {
            Ok(list) => {
                let n = list.a("plugins").iter().filter(|c| c.truthy("installed") && c.truthy("update_available")).count();
                for c in list.a("plugins") { self.set_card(&c); }
                self.paint_all();
                toast(&match n { 0 => "No updates found".to_string(), 1 => "1 update available".to_string(), n => format!("{n} updates available") }, "ok");
            }
            Err(e) => toast(&error_text(&e.message, &e.data), "err"),
        }
    }

    fn add_out(&self, html: &str) { if let Some(o) = dom::qs_in(&self.root, "#plugin-add-out") { dom::set_html(&o, html); } }

    fn sandbox_full(&self) -> bool { self.list.borrow().g("sandbox").s("level") == "full" }

    /// After an upload or a URL install: the new card, open.
    async fn installed_from_add(&self, c: &Value) {
        toast(&format!("{} installed", [c.s("name"), version_text(c)].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ")), "ok");
        redraw(&self.hub, Some(&c.s("id")), Some(c.s("id"))).await;
    }

    /// Upload plugin: the zip checked here (a .zip, its size), the third-party warning, then the multipart upload.
    async fn upload(self: Rc<Self>, file: web_sys::File) {
        let max = { let m = self.list.borrow().i("upload_max_mb"); if m > 0 { m } else { 200 } };
        if let Some(p) = file_problem(&file.name(), file.size(), max) { self.add_out(&error_html(&p)); return; }
        if !crate::ui::confirm_dialog("Install a plugin you add", trust_text(self.sandbox_full()), "Install", false).await { return; }
        let Some(btn) = dom::qs_in(&self.root, "#plugin-upload") else { return };
        dom::set_disabled(&btn, true);
        self.add_out(&format!(r##"<div class="test-result"><span class="spinner"></span><span>Uploading and checking {}…</span></div>"##, esc(&file.name())));
        let Ok(fd) = web_sys::FormData::new() else { return };
        let _ = fd.append_with_blob_and_filename("file", &file, &file.name());
        let r = api::post_form("/plugins/upload", fd).await;
        if !self.alive() { return; }
        dom::set_disabled(&btn, false);
        match r {
            Ok(c) => self.installed_from_add(&c).await,
            Err(e) => self.add_out(&error_html(&error_text(&e.message, &e.data))),
        }
    }

    /// Install from URL or GitHub.
    async fn install_url(self: Rc<Self>, input: String) {
        if let Some(p) = url_problem(&input) { self.add_out(&error_html(p)); return; }
        if !crate::ui::confirm_dialog("Install a plugin you add", trust_text(self.sandbox_full()), "Install", false).await { return; }
        let btn = dom::qs_in(&self.root, "#plugin-url-form button[type=submit]");
        if let Some(b) = &btn { dom::set_disabled(b, true); }
        self.add_out(r##"<div class="test-result"><span class="spinner"></span><span>Downloading and checking…</span></div>"##);
        let r = api::post("/plugins/install-url", json!({ "url": input.trim() })).await;
        if !self.alive() { return; }
        if let Some(b) = &btn { dom::set_disabled(b, false); }
        match r {
            Ok(c) => self.installed_from_add(&c).await,
            Err(e) => self.add_out(&error_html(&error_text(&e.message, &e.data))),
        }
    }
}
