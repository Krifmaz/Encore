//! Settings → Server → Plugins (0.32), the web's settings/plugins.rs: the catalogue of what this server can use
//! and what is installed (GET /api/plugins), with the sandbox level.
//!
//! Each installed plugin is a card: its kind, version, status line, the buttons (Update, Test, Logs, Disable or
//! Enable, Remove, and the auto-update switch), a logs drawer, and its settings, drawn from the fields the server
//! sends for it (GET /api/plugins/{id}/settings, the settings schema's field shape). The plugins that are not
//! installed are listed under Available with Install. "Add a plugin" uploads a zip picked with the file dialog
//! (multipart) or installs from an https address or `owner/repo@ref`, after the third-party warning.
//!
//! The source cards of the Downloads page moved here in 0.32 (streamrip, Torrents & Usenet, Soulseek, yt-dlp with
//! YouTube sign-in as a part of its card), with their blocks from tools.rs: the Tidal sign-in and the streamrip
//! config, the Soulseek status and VPN block, the YouTube sign-in block. A built-in plugin's settings are Encore's
//! own keys, so they are read from and saved with GET/PUT /admin/settings as before (environment locks, the eye
//! and copy buttons, the folder warnings); a plugin's own settings go to PUT /api/plugins/{id}/settings. Old
//! `#/settings?s=streamrip` (… torznab, slskd, ytdlp, youtube) links open this page with that card.

use super::fields;
use super::{env_note, help, input, input_locked, is_masked, link, link_muted, result_line_copy, select, switch, switch_maybe, tools, vs, Opt, Section};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::text_area::text_area;
use crate::widgets::drop_zone::{self, DropZone, Outcome};
use crate::widgets::{self, badge, btn_icon, btn_sm, icon, icon_btn, t, Icon};
use iced::widget::text_editor;
use iced::widget::{button, checkbox, column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// The cards of the Downloads page that became plugin cards in 0.32: (card, plugin). The YouTube sign-in card is
/// a part of yt-dlp's card.
pub const MOVED: &[(&str, &str)] = &[("streamrip", "streamrip"), ("torznab", "torznab"), ("slskd", "slskd"), ("ytdlp", "ytdlp"), ("youtube", "ytdlp")];

/// Other `?s=` names that open the Plugins page: the page itself, the old Download tools card, and the catalogue
/// plugins whose settings are on no other page.
pub const LINKS: &[&str] = &[
    "plugins",
    "tools",
    "qbittorrent",
    "transmission",
    "deluge",
    "rtorrent",
    "torrent-blackhole",
    "sabnzbd",
    "nzbget",
    "usenet-blackhole",
    "translator",
    "jellyfin",
    "plex",
    "navidrome",
];

/// The download client plugins, by the select of the Torrents & Usenet card that chooses them: (select, value,
/// plugin).
pub const CLIENTS: &[(&str, &str, &str)] = &[
    ("torrent_client", "qbittorrent", "qbittorrent"),
    ("torrent_client", "transmission", "transmission"),
    ("torrent_client", "deluge", "deluge"),
    ("torrent_client", "rtorrent", "rtorrent"),
    ("torrent_client", "blackhole", "torrent-blackhole"),
    ("nzb_client", "sabnzbd", "sabnzbd"),
    ("nzb_client", "nzbget", "nzbget"),
    ("nzb_client", "blackhole", "usenet-blackhole"),
];

/// Whether `#/settings?s=<s>` opens the Plugins page.
pub fn opens_plugins(s: &str) -> bool {
    MOVED.iter().any(|(c, _)| *c == s) || LINKS.contains(&s)
}

/// The plugin card a `?s=` name asks for (a moved card's plugin, or a plugin's own id); None for the page itself.
pub fn card_for(s: &str) -> Option<&'static str> {
    if let Some((_, p)) = MOVED.iter().find(|(c, _)| *c == s) {
        return Some(p);
    }
    LINKS.iter().copied().find(|l| *l == s && !matches!(*l, "plugins" | "tools"))
}

// ----- words ---------------------------------------------------------------------------------------------------

/// A plugin's kind in a badge.
pub fn kind_label(kind: &str) -> String {
    match kind {
        "source" => "Source".into(),
        "client" => "Download client".into(),
        "tool" => "Tool".into(),
        "playback" => "Playback".into(),
        "connection" => "Connection".into(),
        "" => "Plugin".into(),
        k => {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// "songs, albums and links": what a source finds (`kinds`).
pub fn kinds_words(kinds: &[Value]) -> String {
    let words: Vec<&str> = kinds
        .iter()
        .filter_map(|k| match k.as_str() {
            Some("track") => Some("songs"),
            Some("album") => Some("albums"),
            Some("url") => Some("links"),
            _ => None,
        })
        .collect();
    match words.len() {
        0 => String::new(),
        1 => words[0].to_string(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// The version a card shows: the installed one (the program's), else the plugin's.
pub fn version_text(c: &Value) -> String {
    if fmt::truthy(c, "installed") && fmt::truthy(c, "installed_version") {
        fmt::s(c, "installed_version")
    } else {
        fmt::s(c, "version")
    }
}

/// What the page is doing to a plugin, in a few words.
pub fn busy_words(what: &str) -> &'static str {
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
    let name = if fmt::truthy(after, "name") { fmt::s(after, "name") } else { fmt::s(before, "name") };
    let v = version_text(after);
    match act {
        "install" => {
            if fmt::truthy(after, "installed_version") {
                format!("{name} {v} installed")
            } else {
                format!("{name} installed")
            }
        }
        "update" => {
            if fmt::s(before, "installed_version") != fmt::s(after, "installed_version") {
                format!("{name} updated to {v}")
            } else if fmt::truthy(after, "last_error") {
                // the newest release could not run here (it needs a newer Python): the card's note says why
                format!("{name} stays on {v}")
            } else {
                format!("{name} is up to date")
            }
        }
        "remove" => format!("{name} removed"),
        "enable" => format!("{name} enabled"),
        "disable" => format!("{name} disabled"),
        _ => "Done".into(),
    }
}

/// After Check for updates: "No updates found", "1 update available", "3 updates available".
pub fn check_words(list: &Value) -> String {
    let n = fmt::items(&list["plugins"]).iter().filter(|c| fmt::truthy(c, "installed") && fmt::truthy(c, "update_available")).count();
    match n {
        0 => "No updates found".into(),
        1 => "1 update available".into(),
        n => format!("{n} updates available"),
    }
}

/// A failed call as one text: the server's sentence and, for a refused manifest, each problem on its own line.
pub fn error_text(message: &str, data: &Value) -> String {
    let problems: Vec<String> = fmt::items(&data["problems"]).iter().map(vs).filter(|p| !p.is_empty()).collect();
    let head = if message.trim().is_empty() { "Failed".to_string() } else { message.to_string() };
    if problems.is_empty() {
        head
    } else {
        format!("{head}\n{}", problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"))
    }
}

fn api_error_text(e: &ApiError) -> String {
    error_text(&e.message, &e.data)
}

/// What a Test answered (`{ok, message}` or `{ok: false, error, code, retry_after}`), as (ok, words).
pub fn test_answer(r: &Value) -> (bool, String) {
    let ok = fmt::truthy(r, "ok");
    let mut msg = if fmt::truthy(r, "message") { fmt::s(r, "message") } else { fmt::s(r, "error") };
    if !ok && fmt::s(r, "code") == "rate_limited" && fmt::f(r, "retry_after") > 0.0 {
        msg = format!("{msg} – try again in {}", tools::span(fmt::f(r, "retry_after")));
    }
    if msg.trim().is_empty() {
        msg = if ok { "Test passed".into() } else { "Test failed".into() };
    }
    (ok, msg)
}

/// Why an address for Install from URL cannot be used, before it is sent.
pub fn url_problem(input: &str) -> Option<&'static str> {
    let s = input.trim();
    if s.is_empty() {
        return Some("Enter an https address, or owner/repo@ref for GitHub");
    }
    if s.to_ascii_lowercase().starts_with("http://") {
        return Some("Plugins install only from https addresses");
    }
    None
}

/// Why a picked file cannot be uploaded, before it is sent.
pub fn file_problem(name: &str, bytes: f64, max_mb: i64) -> Option<String> {
    if !name.to_ascii_lowercase().ends_with(".zip") {
        return Some("A plugin is uploaded as a .zip file".into());
    }
    if bytes > (max_mb.max(1) as f64) * 1024.0 * 1024.0 {
        return Some(format!("The plugin zip is larger than {max_mb} MB"));
    }
    None
}

/// An address the page may open: http(s) only (a plugin's homepage is the plugin's own text).
fn safe_link(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://")
}

/// The question before Remove: (title, words).
pub fn remove_words(name: &str, catalog: bool) -> (String, String) {
    let text = if catalog {
        format!("Remove {name}? Encore stops using it until you install it again. What the plugin downloaded is deleted; your settings for it are kept.")
    } else {
        format!("Remove {name}? Its program, data, log and settings are deleted.")
    };
    (format!("Remove {name}"), text)
}

/// The Test button's words: those of the card the plugin's settings were before 0.32, or "Test".
pub fn test_label(id: &str) -> &'static str {
    match fields::card(id) {
        Some(c) if c.test.is_some() => c.test_label.unwrap_or("Test connection"),
        _ => "Test",
    }
}

/// The words of the auto-update switch.
pub fn auto_words(id: &str) -> &'static str {
    if id == "ytdlp" {
        "Update automatically (recommended – sites change often)"
    } else {
        "Update automatically when a new release is out"
    }
}

/// The status line of an installed plugin: (tone, words) with tone "ok", "err" or "" (muted). None for a plugin
/// that is not installed.
pub fn status_words(c: &Value, acting: Option<&str>) -> Option<(&'static str, String)> {
    if let Some(b) = acting.map(str::to_string).or_else(|| c["busy"].as_str().map(String::from)) {
        return Some(("", busy_words(&b).to_string()));
    }
    if !fmt::truthy(c, "installed") {
        return None;
    }
    if !fmt::truthy(c, "enabled") {
        return Some(("", "Disabled – Encore does not use it until you enable it again".into()));
    }
    let st = &c["status"];
    if !st.is_object() {
        return Some(("", "Checking…".into()));
    }
    if fmt::truthy(st, "ok") {
        Some(("ok", if fmt::truthy(st, "message") { fmt::s(st, "message") } else { "Ready".into() }))
    } else {
        Some(("err", if fmt::truthy(st, "message") { fmt::s(st, "message") } else { "Not working".into() }))
    }
}

/// The dot beside an installed plugin's name: "ok", "err", "warn" (it works but needs setting up: not signed in,
/// no address yet – `status.signed_in` false) or "" (off, or not known yet).
pub fn dot_tone(c: &Value) -> &'static str {
    if !fmt::truthy(c, "installed") || !fmt::truthy(c, "enabled") {
        return "";
    }
    let st = &c["status"];
    if fmt::truthy(c, "last_error") || (st.is_object() && !fmt::truthy(st, "ok")) {
        "err"
    } else if st["signed_in"] == json!(false) {
        "warn"
    } else if fmt::truthy(st, "ok") {
        "ok"
    } else {
        ""
    }
}

/// The line about a plugin: what it finds, its sandbox and permissions (a third-party plugin) and licence (the
/// homepage is a link beside it).
pub fn about_words(c: &Value) -> String {
    let mut parts: Vec<String> = vec![];
    if fmt::s(c, "kind") == "source" {
        let finds = kinds_words(&fmt::items(&c["kinds"]));
        if !finds.is_empty() {
            parts.push(format!("Finds {finds}{}", if fmt::truthy(c, "lossless") { ", lossless" } else { "" }));
        }
    }
    if fmt::s(c, "trust") == "third-party" {
        if fmt::truthy(c, "sandbox") {
            parts.push(format!("Sandbox: {}", fmt::s(c, "sandbox")));
        }
        let hosts: Vec<String> = fmt::items(&c["hosts"]).iter().map(vs).filter(|h| !h.is_empty()).collect();
        parts.push(if !fmt::truthy(c, "network") {
            "No network".into()
        } else if hosts.is_empty() {
            "Network: any address".into()
        } else {
            format!("Network: {}", hosts.join(", "))
        });
        if fmt::truthy(c, "reads_library") {
            parts.push("Reads your library (read only)".into());
        }
    }
    if fmt::truthy(c, "license") {
        parts.push(format!("Licence {}", fmt::s(c, "license")));
    }
    parts.join(" · ")
}

/// A download client's card: which choice of the Torrents & Usenet card sends downloads to it, and whether it is
/// chosen now. (tone, words): "" in use, "warn" not in use.
pub fn client_note(name: &str, select_label: &str, option_label: &str, chosen: bool, chosen_label: &str) -> (&'static str, String) {
    if chosen {
        ("", format!("In use: {select_label} is set to {option_label} on the Torrents & Usenet card."))
    } else {
        (
            "warn",
            format!(
                "Not in use: {select_label} is set to {} on the Torrents & Usenet card. Choose {option_label} there to send downloads to {name}.",
                if chosen_label.is_empty() { "something else" } else { chosen_label }
            ),
        )
    }
}

/// The torznab card's line for a chosen client whose plugin is not ready, and the action that fixes it.
pub fn chosen_client_line(select: &str, chosen: &str, c: &Value) -> Option<(String, &'static str)> {
    if !fmt::truthy(c, "installed") {
        Some((format!("{select}: {chosen} has a plugin of its own, not installed yet."), "install"))
    } else if !fmt::truthy(c, "enabled") {
        Some((format!("{select}: the {chosen} plugin is disabled."), "enable"))
    } else {
        None
    }
}

/// A select's label and the words of one of its options, from the settings table (`torrent_client` =
/// `rtorrent` → ("Torrent client", "rTorrent / ruTorrent")).
fn option_label(key: &str, value: &str) -> Option<(&'static str, String)> {
    let f = fields::SECTIONS.iter().flat_map(|c| c.fields.iter()).find(|f| f.key == key)?;
    let words = f.options.iter().find(|(o, _)| *o == value).map(|(_, l)| l.to_string()).unwrap_or_else(|| value.to_string());
    Some((f.label, words))
}

// ----- the settings card of a plugin ----------------------------------------------------------------------------

/// A field of a plugin's settings card, as GET /api/plugins/{id}/settings sends it (the settings schema's field
/// shape, plus `part`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PField {
    pub key: String,
    pub label: String,
    pub help: String,
    /// text, password, number, bool, select, checks, textarea, path or url
    pub kind: String,
    pub options: Vec<(String, String)>,
    /// `key=value` or `key!=value`: (key, value, equal); a rule naming a select of another card is left out
    pub when: Option<(String, String, bool)>,
    /// the server never sends the saved value back
    pub secret: bool,
    pub full: bool,
    /// the heading the field and the ones after it sit under ("" for none)
    pub part: String,
}

/// `key=value` or `key!=value`.
fn parse_when(w: &str) -> Option<(String, String, bool)> {
    let (k, v, eq) = match w.split_once("!=") {
        Some((k, v)) => (k, v, false),
        None => {
            let (k, v) = w.split_once('=')?;
            (k, v, true)
        }
    };
    let (k, v) = (k.trim(), v.trim());
    (!k.is_empty() && !v.is_empty()).then(|| (k.to_string(), v.to_string(), eq))
}

impl PField {
    pub fn from_json(f: &Value) -> PField {
        let kind = fmt::s(f, "type");
        PField {
            key: fmt::s(f, "key"),
            label: if fmt::truthy(f, "label") { fmt::s(f, "label") } else { fmt::s(f, "key") },
            help: fmt::s(f, "help"),
            kind: if kind.is_empty() { "text".into() } else { kind.clone() },
            options: fmt::items(&f["options"]).iter().map(|o| (vs(&o["value"]), vs(&o["label"]))).collect(),
            when: f["when"].as_str().and_then(parse_when),
            secret: fmt::truthy(f, "secret") || kind == "password",
            full: fmt::truthy(f, "full"),
            part: fmt::s(f, "part"),
        }
    }

    /// A password or a secret text area: the input starts empty and an empty one keeps the saved value.
    pub fn is_secret(&self) -> bool {
        self.kind == "password" || self.secret
    }

    /// The field takes the whole row of the form grid.
    pub fn spans(&self) -> bool {
        self.full || self.kind == "textarea" || self.kind == "checks"
    }
}

/// The values a card's fields show, with `_meta` (where each comes from). A built-in plugin's keys are Encore's
/// settings: GET /admin/settings (`settings`) has them with their `_meta`; a key it lacks is taken from the
/// plugin's answer. A plugin's own settings are only in its answer (saved ones show as saved, none locked).
pub fn card_values(answer: &Value, settings: &Value) -> Value {
    let builtin = fmt::truthy(answer, "builtin");
    let mut values = if builtin && settings.is_object() { settings.as_object().cloned().unwrap_or_default() } else { Map::new() };
    let mut meta = values.get("_meta").and_then(|m| m.as_object().cloned()).unwrap_or_default();
    for f in fmt::items(&answer["fields"]) {
        let key = fmt::s(&f, "key");
        if !values.contains_key(&key) {
            values.insert(key.clone(), answer["values"][&key].clone());
        }
        if !meta.contains_key(&key) {
            let source = if fmt::truthy(&f, "env_locked") {
                "env"
            } else if builtin {
                "default"
            } else {
                "ui"
            };
            meta.insert(key, json!({ "source": source, "env": f["env"] }));
        }
    }
    values.insert("_meta".into(), Value::Object(meta));
    Value::Object(values)
}

/// A plugin's settings card while the page is open: the server's answer, the values its fields show and what was
/// typed.
#[derive(Debug, Default)]
pub struct Form {
    /// GET /api/plugins/{id}/settings: `{id, builtin, settings_on, fields, values}`
    answer: Value,
    /// why the settings could not be read
    error: String,
    fields: Vec<PField>,
    /// [`card_values`]: the values with `_meta`
    values: Value,
    edits: HashMap<String, String>,
    bools: HashMap<String, bool>,
    checks: HashMap<String, Vec<String>>,
    areas: HashMap<String, text_editor::Content>,
    /// plain text areas typed into: they keep their text when the values come again
    typed: HashSet<String>,
    /// secrets shown in clear
    shown: HashSet<String>,
    /// secrets fetched from the server to show
    revealed: HashSet<String>,
    saving: bool,
    /// lines under the card after a save (the server's folder warnings)
    flash: Vec<(bool, String)>,
}

impl Form {
    fn new(answer: Value, settings: &Value) -> Form {
        let mut f = Form::default();
        f.take(answer, settings);
        f
    }

    fn failed(error: String) -> Form {
        Form { error, ..Form::default() }
    }

    fn builtin(&self) -> bool {
        fmt::truthy(&self.answer, "builtin")
    }

    /// A new answer from the server (after a load or a save of a plugin's own settings).
    fn take(&mut self, answer: Value, settings: &Value) {
        let keys: Vec<String> = fmt::items(&answer["fields"]).iter().map(|f| fmt::s(f, "key")).collect();
        let mut part = String::new();
        self.fields = fmt::items(&answer["fields"])
            .iter()
            .map(|f| {
                let mut p = PField::from_json(f);
                // a part holds its field and the ones after it
                if p.part.is_empty() {
                    p.part = part.clone();
                } else {
                    part = p.part.clone();
                }
                if p.when.as_ref().is_some_and(|(k, _, _)| !keys.contains(k)) {
                    p.when = None;
                }
                p
            })
            .collect();
        self.answer = answer;
        self.error.clear();
        self.refresh(settings);
    }

    /// The values again (Encore's settings changed: a save of this card or another): text areas show what is saved.
    fn refresh(&mut self, settings: &Value) {
        self.values = card_values(&self.answer, settings);
        for f in self.fields.iter().filter(|f| f.kind == "textarea") {
            if f.is_secret() {
                self.areas.entry(f.key.clone()).or_default();
            } else if !self.typed.contains(&f.key) {
                self.areas.insert(f.key.clone(), text_editor::Content::with_text(&vs(&self.values[&f.key])));
            }
        }
    }

    fn value(&self, key: &str) -> &Value {
        &self.values[key]
    }

    fn locked(&self, key: &str) -> bool {
        self.values["_meta"][key]["source"].as_str() == Some("env")
    }

    /// The value an input shows: what was typed, else the saved value.
    fn current(&self, key: &str) -> String {
        self.edits.get(key).cloned().unwrap_or_else(|| vs(self.value(key)))
    }

    fn checked(&self, f: &PField) -> Vec<String> {
        match self.checks.get(&f.key) {
            Some(c) => c.clone(),
            None => vs(self.value(&f.key)).split(',').map(fields::type_key).filter(|x| !x.is_empty()).collect(),
        }
    }

    fn visible(&self, f: &PField) -> bool {
        match &f.when {
            Some((k, v, eq)) => (self.current(k) == *v) == *eq,
            None => true,
        }
    }

    /// The body of Save, as the settings pages build it: every field the environment does not set; secrets only
    /// when something new was typed (an empty box keeps the saved value).
    pub fn save_body(&self) -> Map<String, Value> {
        let mut body = Map::new();
        for f in &self.fields {
            if self.locked(&f.key) {
                continue;
            }
            match f.kind.as_str() {
                "checks" => {
                    let on = self.checked(f);
                    let joined: Vec<&str> = f.options.iter().filter(|(v, _)| on.contains(&fields::type_key(v))).map(|(v, _)| v.as_str()).collect();
                    body.insert(f.key.clone(), Value::String(joined.join(",")));
                }
                "bool" => {
                    let on = self.bools.get(&f.key).copied().unwrap_or_else(|| super::truthy(self.value(&f.key)));
                    body.insert(f.key.clone(), Value::Bool(on));
                }
                "textarea" => {
                    let text = self.areas.get(&f.key).map(|c| c.text()).unwrap_or_default();
                    if f.is_secret() && (text.trim().is_empty() || is_masked(&text) || self.revealed.contains(&f.key)) {
                        continue;
                    }
                    body.insert(f.key.clone(), Value::String(text.trim().to_string()));
                }
                _ if f.is_secret() => {
                    let text = self.edits.get(&f.key).cloned().unwrap_or_default();
                    if text.trim().is_empty() || is_masked(&text) || self.revealed.contains(&f.key) {
                        continue;
                    }
                    body.insert(f.key.clone(), Value::String(text.trim().to_string()));
                }
                _ => {
                    let text = self.current(&f.key);
                    if is_masked(&text) {
                        continue;
                    }
                    body.insert(f.key.clone(), Value::String(text));
                }
            }
        }
        body
    }

    /// Forget what was typed (the saved values show again).
    fn settle(&mut self) {
        self.edits.clear();
        self.bools.clear();
        self.checks.clear();
        self.shown.clear();
        self.revealed.clear();
        self.typed.clear();
        for f in self.fields.iter().filter(|f| f.kind == "textarea") {
            let saved = if f.is_secret() { String::new() } else { vs(&self.values[&f.key]) };
            self.areas.insert(f.key.clone(), text_editor::Content::with_text(&saved));
        }
    }

    /// The eye and copy buttons ask GET /admin/settings/secret/<key>: Encore's own keys only, never the WireGuard
    /// key.
    fn reveals(&self, f: &PField) -> bool {
        f.is_secret() && self.builtin() && f.key != "vpn_wireguard_config" && super::truthy(self.value(&f.key))
    }

    /// The save as one call: (path, body, whether it goes to Encore's settings).
    fn save_call(&self, id: &str) -> (String, Value, bool) {
        let body = Value::Object(self.save_body());
        if self.builtin() {
            ("/admin/settings".into(), body, true)
        } else {
            (plugin_path(id, "settings"), json!({ "values": body }), false)
        }
    }
}

// ----- the page -------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum Dialog {
    /// Remove a plugin: the question's title and words are made when it is asked.
    Remove { id: String, title: String, text: String },
    /// The third-party warning before a plugin from a zip or an address is installed.
    Trust(Add),
}

/// What Add a plugin installs.
#[derive(Clone, Debug, PartialEq)]
pub enum Add {
    Upload(PathBuf),
    Url(String),
}

/// The logs drawer of one card.
#[derive(Debug, Default)]
pub struct Logs {
    file: String,
    text: String,
    content: text_editor::Content,
    loading: bool,
}

/// The answer of a Test and the quiet save before it: (the save's answer, when there was one; the test's).
pub type Tested = Result<(Option<Value>, Result<Value, ApiError>), ApiError>;

#[derive(Debug, Default)]
pub struct State {
    /// GET /api/plugins (None while it loads)
    list: Option<Value>,
    /// the server has no plugins (Encore before 0.32): its sources are under Downloads
    missing: bool,
    error: String,
    /// GET /admin/settings: the values of the built-in plugins' keys, with `_meta`
    settings: Value,
    /// unfolded cards
    open: HashSet<String>,
    forms: HashMap<String, Form>,
    /// what this page is doing to a plugin ("install", "update", "enable", "disable", "remove", "test")
    acting: HashMap<String, String>,
    /// the line under a card's buttons: a Test's answer or why an action failed
    outs: HashMap<String, (bool, String)>,
    /// the answer of a part's Test (YouTube sign-in's Test cookies), by plugin
    part_outs: HashMap<String, (bool, String)>,
    part_testing: HashSet<String>,
    logs: HashMap<String, Logs>,
    checking: bool,
    url: String,
    adding: bool,
    /// the line under Add a plugin: (None while it works, Some(false) a failure)
    add_out: Option<(Option<bool>, String)>,
    dialog: Option<Dialog>,
    /// plugin zips dragged onto the window for *Upload plugin*
    drop: DropZone,
    /// the live blocks of the source cards (Tidal sign-in, streamrip config, Soulseek, VPN, YouTube sign-in)
    pub tools: tools::State,
}

#[derive(Clone, Debug)]
pub enum Message {
    List(Result<Value, ApiError>),
    Values(Result<Value, ApiError>),
    /// GET /api/plugins/{id}: the card with its status asked now
    One(String, Result<Value, ApiError>),
    Settings(String, Result<Value, ApiError>),
    Toggle(String),
    /// install, update, enable, disable or remove (remove asks first)
    Act(String, &'static str),
    Remove,
    Done(String, &'static str, Value, Result<Value, ApiError>),
    CheckAll,
    Checked(Result<Value, ApiError>),
    Test(String),
    Tested(String, Tested),
    /// A part's Test: the plugin and POST /system/test/<test>.
    PartTest(String, &'static str),
    PartTested(String, Tested),
    Auto(String, bool),
    AutoSaved(Result<Value, ApiError>),
    Logs(String),
    LogsRefresh(String),
    LogsLoaded(String, Result<Value, ApiError>),
    LogText(String, text_editor::Action),
    LogCopy(String),
    PickUpload,
    UploadPicked(Option<PathBuf>),
    /// a file dragged over or dropped on the window: a dropped plugin zip is uploaded
    Drop(drop_zone::Event),
    Url(String),
    InstallUrl,
    Install(Add),
    Added(Result<Value, ApiError>),
    Edit(String, String, String),
    Bool(String, String, bool),
    Check(String, String, String, bool),
    Area(String, String, text_editor::Action),
    Reveal(String, String),
    Revealed(String, String, Result<Value, ApiError>),
    Hide(String, String),
    CopySecret(String, String),
    Copied(Result<Value, ApiError>),
    Clear(String, String),
    Save(String),
    Saved(String, &'static str, Result<Value, ApiError>),
    VpnSaved(Result<Value, ApiError>),
    Tools(tools::Message),
    Go(Section),
    OpenUrl(String),
    CopyText(String),
    /// Every 3 s while a plugin is busy on the server or the Tidal sign-in waits.
    Tick,
    CloseDialog,
}

/// `encodeURIComponent` of a plugin id (ids are `a-z0-9-`; anything else is escaped).
fn enc(id: &str) -> String {
    id.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn plugin_path(id: &str, what: &str) -> String {
    format!("/plugins/{}/{what}", enc(id))
}

fn load_list(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/plugins").await }, Message::List)
}

fn load_one(ctx: &Ctx, id: &str) -> Task<Message> {
    let api = ctx.api.clone();
    let id = id.to_string();
    let path = format!("/plugins/{}", enc(&id));
    Task::perform(async move { api.get(&path).await }, move |r| Message::One(id.clone(), r))
}

fn load_settings(ctx: &Ctx, id: &str) -> Task<Message> {
    let api = ctx.api.clone();
    let id = id.to_string();
    let path = plugin_path(&id, "settings");
    Task::perform(async move { api.get(&path).await }, move |r| Message::Settings(id.clone(), r))
}

fn load_logs(ctx: &Ctx, id: &str) -> Task<Message> {
    let api = ctx.api.clone();
    let id = id.to_string();
    let path = format!("{}?lines=200", plugin_path(&id, "logs"));
    Task::perform(async move { api.get(&path).await }, move |r| Message::LogsLoaded(id.clone(), r))
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let tools = std::mem::take(&mut state.tools);
    *state = State { tools, ..State::default() };
    // a link to one plugin (`#/settings?s=slskd`) opens with its card unfolded
    if let Some(f) = super::take_focus() {
        state.open.insert(f.to_string());
    }
    let api = ctx.api.clone();
    Task::batch([
        load_list(ctx),
        Task::perform(async move { api.get("/admin/settings").await }, Message::Values),
        tools::open_sources(&mut state.tools, ctx).map(Message::Tools),
    ])
}

/// The cards of GET /api/plugins.
fn cards(state: &State) -> Vec<&Value> {
    state.list.as_ref().and_then(|l| l["plugins"].as_array()).map(|a| a.iter().collect()).unwrap_or_default()
}

fn card_of(state: &State, id: &str) -> Value {
    cards(state).into_iter().find(|c| fmt::s(c, "id") == id).cloned().unwrap_or_else(|| json!({ "id": id }))
}

/// Put a card the server sent in the list in place of the old one.
fn set_card(state: &mut State, card: Value) {
    let id = fmt::s(&card, "id");
    if id.is_empty() || !card.is_object() {
        return;
    }
    if let Some(list) = state.list.as_mut().and_then(|l| l["plugins"].as_array_mut()) {
        if let Some(slot) = list.iter_mut().find(|c| fmt::s(c, "id") == id) {
            *slot = card;
        }
    }
}

/// Whether the card draws its settings here (not on another page).
fn has_form(card: &Value) -> bool {
    fmt::truthy(card, "installed") && fmt::truthy(card, "has_settings") && !fmt::truthy(card, "settings_on")
}

/// Any plugin busy on the server (another admin's install, the first start's installs): the page looks again.
fn anything_busy(state: &State) -> bool {
    cards(state).iter().any(|c| fmt::truthy(c, "busy"))
}

/// The page after the list (again): the settings of the installed plugins drawn here, and (`statuses`) the status
/// of every enabled one.
fn after_list(state: &State, ctx: &Ctx, statuses: bool) -> Task<Message> {
    let mut tasks = vec![];
    for c in cards(state) {
        let id = fmt::s(c, "id");
        if has_form(c) && !state.forms.contains_key(&id) {
            tasks.push(load_settings(ctx, &id));
        }
        if statuses && fmt::truthy(c, "installed") && fmt::truthy(c, "enabled") {
            tasks.push(load_one(ctx, &id));
        }
    }
    Task::batch(tasks)
}

/// Encore's settings came again (a save): every built-in card shows them. The folder warnings of a save are
/// given back.
fn take_settings(state: &mut State, mut v: Value) -> Vec<String> {
    let warnings: Vec<String> = fmt::items(&v["_warnings"]).iter().map(|w| fmt::s(w, "message")).filter(|m| !m.is_empty()).collect();
    if let Some(o) = v.as_object_mut() {
        o.remove("_warnings");
    }
    state.settings = v;
    for f in state.forms.values_mut().filter(|f| f.builtin()) {
        f.refresh(&state.settings);
    }
    warnings
}

/// A card's quiet save before its Test, as one call: (path, body, to Encore's settings).
fn quiet_save(state: &State, id: &str) -> Option<(String, Value, bool)> {
    state.forms.get(id).filter(|f| f.error.is_empty() && !f.fields.is_empty()).map(|f| f.save_call(id))
}

/// After a quiet save answered: Encore's settings or the plugin's own answer.
fn saved_quietly(state: &mut State, id: &str, saved: Option<Value>) {
    let Some(v) = saved else { return };
    if v.get("fields").is_some() {
        let settings = state.settings.clone();
        if let Some(f) = state.forms.get_mut(id) {
            f.take(v, &settings);
        }
    } else {
        take_settings(state, v);
    }
    if let Some(f) = state.forms.get_mut(id) {
        f.settle();
    }
}

/// Save the card quietly (when it has settings), then POST `test`.
fn save_then(state: &State, ctx: &Ctx, id: &str, test: String) -> Task<Tested> {
    let save = quiet_save(state, id);
    let api = ctx.api.clone();
    Task::perform(
        async move {
            let saved = match save {
                Some((path, body, _)) => Some(api.put(&path, body).await?),
                None => None,
            };
            Ok((saved, api.post_slow(&test, json!({})).await))
        },
        |r| r,
    )
}

/// Send install, update, enable, disable or remove; the card says what it is doing until the answer.
fn start(state: &mut State, ctx: &Ctx, id: String, act: &'static str, before: Value) -> Task<Message> {
    state.acting.insert(id.clone(), act.to_string());
    state.outs.remove(&id);
    let api = ctx.api.clone();
    let path = plugin_path(&id, act);
    Task::perform(async move { api.post_slow(&path, json!({})).await }, move |r| Message::Done(id.clone(), act, before.clone(), r))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::List(Ok(v)) => {
            if !v["plugins"].is_array() {
                // an older server answers this address with its web page
                state.missing = true;
                state.list = Some(json!({ "plugins": [] }));
                return Task::none();
            }
            let first = state.list.is_none();
            state.error.clear();
            state.list = Some(v);
            return after_list(state, ctx, first);
        }
        Message::List(Err(e)) => {
            if e.status == 404 {
                state.missing = true;
                state.list = Some(json!({ "plugins": [] }));
            } else if state.list.is_none() {
                state.error = api_error_text(&e);
            }
        }
        Message::Values(Ok(v)) => {
            take_settings(state, v);
        }
        Message::Values(Err(_)) => {}
        Message::One(id, r) => match r {
            Ok(card) => set_card(state, card),
            // removed elsewhere (another admin, an upload of the same id that failed half way): the list again
            Err(e) if e.status == 404 && fmt::truthy(&card_of(state, &id), "installed") => return load_list(ctx),
            Err(_) => {}
        },
        Message::Settings(id, Ok(v)) => {
            let form = Form::new(v, &state.settings);
            state.forms.insert(id, form);
        }
        Message::Settings(id, Err(e)) => {
            state.forms.insert(id, Form::failed(api_error_text(&e)));
        }
        Message::Toggle(id) => {
            if !state.open.remove(&id) {
                state.open.insert(id);
            }
        }
        Message::Act(id, act) => {
            if state.acting.contains_key(&id) {
                return Task::none();
            }
            let before = card_of(state, &id);
            if act == "remove" {
                let name = if fmt::truthy(&before, "name") { fmt::s(&before, "name") } else { id.clone() };
                let (title, text) = remove_words(&name, fmt::s(&before, "origin") == "catalog");
                state.dialog = Some(Dialog::Remove { id, title, text });
                return Task::none();
            }
            return start(state, ctx, id, act, before);
        }
        Message::Remove => {
            if let Some(Dialog::Remove { id, .. }) = state.dialog.take() {
                let before = card_of(state, &id);
                return start(state, ctx, id, "remove", before);
            }
        }
        Message::Done(id, act, before, r) => {
            state.acting.remove(&id);
            match r {
                Ok(c) => {
                    ctx.toast_ok(done_words(act, &before, &c));
                    match act {
                        "install" => {
                            // the new card, open, where it now is (Installed, above the list it came from)
                            state.open.insert(id.clone());
                            state.forms.remove(&id);
                            set_card(state, c);
                            return Task::batch([load_list(ctx), load_one(ctx, &id)]);
                        }
                        "remove" => {
                            state.open.remove(&id);
                            state.forms.remove(&id);
                            state.logs.remove(&id);
                            set_card(state, c);
                            return load_list(ctx);
                        }
                        _ => {
                            set_card(state, c);
                            if act != "disable" {
                                return load_one(ctx, &id);
                            }
                        }
                    }
                }
                Err(e) => {
                    state.outs.insert(id.clone(), (false, api_error_text(&e)));
                    // the card as the server has it now (its last error)
                    return load_one(ctx, &id);
                }
            }
        }
        Message::CheckAll => {
            state.checking = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_slow("/plugins/check", json!({})).await }, Message::Checked);
        }
        Message::Checked(r) => {
            state.checking = false;
            match r {
                Ok(list) if list["plugins"].is_array() => {
                    ctx.toast_ok(check_words(&list));
                    for c in fmt::items(&list["plugins"]) {
                        set_card(state, c);
                    }
                }
                Ok(_) => {}
                Err(e) => ctx.toast_err(api_error_text(&e)),
            }
        }
        Message::Test(id) => {
            if state.acting.contains_key(&id) {
                return Task::none();
            }
            state.acting.insert(id.clone(), "test".into());
            state.outs.insert(id.clone(), (true, "Testing…".into()));
            let i2 = id.clone();
            return save_then(state, ctx, &id, plugin_path(&id, "test")).map(move |r| Message::Tested(i2.clone(), r));
        }
        Message::Tested(id, r) => {
            state.acting.remove(&id);
            let line = match r {
                Ok((saved, tested)) => {
                    saved_quietly(state, &id, saved);
                    match tested {
                        Ok(v) => test_answer(&v),
                        Err(e) => (false, api_error_text(&e)),
                    }
                }
                Err(e) => test_answer(&json!({ "ok": false, "error": api_error_text(&e) })),
            };
            state.outs.insert(id.clone(), line);
            return load_one(ctx, &id);
        }
        Message::PartTest(id, test) => {
            if state.part_testing.contains(&id) {
                return Task::none();
            }
            state.part_testing.insert(id.clone());
            state.part_outs.insert(id.clone(), (true, "Testing…".into()));
            let i2 = id.clone();
            return save_then(state, ctx, &id, format!("/system/test/{test}")).map(move |r| Message::PartTested(i2.clone(), r));
        }
        Message::PartTested(id, r) => {
            state.part_testing.remove(&id);
            let line = match r {
                Ok((saved, tested)) => {
                    saved_quietly(state, &id, saved);
                    match tested {
                        Ok(v) => {
                            let ok = fmt::truthy(&v, "ok");
                            let m = fmt::s(&v, "message");
                            (ok, if !m.is_empty() { m } else if ok { "Connected".into() } else { "Failed".into() })
                        }
                        Err(e) => (false, if e.message.is_empty() { "Test failed".into() } else { e.message }),
                    }
                }
                Err(e) => (false, api_error_text(&e)),
            };
            state.part_outs.insert(id, line);
        }
        Message::Auto(id, on) => {
            let api = ctx.api.clone();
            let path = plugin_path(&id, "auto-update");
            return Task::perform(async move { api.post(&path, json!({ "on": on })).await }, Message::AutoSaved);
        }
        Message::AutoSaved(Ok(c)) => {
            set_card(state, c);
            ctx.toast_ok("Saved");
        }
        Message::AutoSaved(Err(e)) => ctx.toast_err(e.message),
        Message::Logs(id) => {
            // the drawer opens (and reads the log) or closes
            if state.logs.remove(&id).is_some() {
                return Task::none();
            }
            state.logs.insert(id.clone(), Logs { loading: true, ..Logs::default() });
            return load_logs(ctx, &id);
        }
        Message::LogsRefresh(id) => {
            if let Some(l) = state.logs.get_mut(&id) {
                l.loading = true;
            }
            return load_logs(ctx, &id);
        }
        Message::LogsLoaded(id, r) => {
            if let Some(l) = state.logs.get_mut(&id) {
                l.loading = false;
                match r {
                    Ok(v) => {
                        let lines: Vec<String> = fmt::items(&v["lines"]).iter().map(vs).collect();
                        l.text = if lines.is_empty() { "The log is empty.".into() } else { lines.join("\n") };
                        l.file = fmt::s(&v, "file");
                    }
                    Err(e) => l.text = api_error_text(&e),
                }
                l.content = text_editor::Content::with_text(&l.text);
                // the newest lines are at the end
                l.content.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
            }
        }
        Message::LogText(id, a) => {
            if let Some(l) = state.logs.get_mut(&id) {
                if !a.is_edit() {
                    l.content.perform(a);
                }
            }
        }
        Message::LogCopy(id) => {
            if let Some(l) = state.logs.get(&id) {
                ctx.copy_text(l.text.clone());
            }
        }
        Message::PickUpload => {
            return Task::perform(
                async move { rfd::AsyncFileDialog::new().set_title("Upload plugin").add_filter("Plugin zip", PLUGIN_FILES.exts).pick_file().await.map(|f| f.path().to_path_buf()) },
                Message::UploadPicked,
            );
        }
        Message::UploadPicked(None) => {}
        Message::Drop(ev) => {
            // like *Upload plugin*: not while a plugin is being added or a dialog asks something
            if state.adding || state.dialog.is_some() || state.list.is_none() {
                state.drop.hovering = false;
                return Task::none();
            }
            match state.drop.handle(ev, &PLUGIN_FILES) {
                Outcome::Take(path) => return Task::done(Message::UploadPicked(Some(path))),
                Outcome::Refused(m) => ctx.toast_err(m),
                Outcome::Nothing => {}
            }
        }
        Message::UploadPicked(Some(path)) => {
            let max = state.list.as_ref().map(|l| fmt::i(l, "upload_max_mb")).filter(|m| *m > 0).unwrap_or(200);
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len() as f64).unwrap_or(0.0);
            if let Some(p) = file_problem(&name, size, max) {
                state.add_out = Some((Some(false), p));
                return Task::none();
            }
            state.dialog = Some(Dialog::Trust(Add::Upload(path)));
        }
        Message::Url(s) => state.url = s,
        Message::InstallUrl => {
            if state.adding {
                return Task::none();
            }
            if let Some(p) = url_problem(&state.url) {
                state.add_out = Some((Some(false), p.to_string()));
                return Task::none();
            }
            state.dialog = Some(Dialog::Trust(Add::Url(state.url.trim().to_string())));
        }
        Message::Install(add) => {
            state.dialog = None;
            state.adding = true;
            let api = ctx.api.clone();
            return match add {
                Add::Upload(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    state.add_out = Some((None, format!("Uploading and checking {name}…")));
                    Task::perform(
                        async move {
                            let part = crate::api::Api::file_part(&path).await?;
                            api.post_form_slow("/plugins/upload", reqwest::multipart::Form::new().part("file", part)).await
                        },
                        Message::Added,
                    )
                }
                Add::Url(url) => {
                    state.add_out = Some((None, "Downloading and checking…".into()));
                    Task::perform(async move { api.post_slow("/plugins/install-url", json!({ "url": url })).await }, Message::Added)
                }
            };
        }
        Message::Added(r) => {
            state.adding = false;
            match r {
                Ok(c) => {
                    let words = [fmt::s(&c, "name"), version_text(&c)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
                    ctx.toast_ok(format!("{words} installed"));
                    state.add_out = None;
                    state.url.clear();
                    let id = fmt::s(&c, "id");
                    state.open.insert(id.clone());
                    state.forms.remove(&id);
                    return Task::batch([load_list(ctx), load_one(ctx, &id)]);
                }
                Err(e) => state.add_out = Some((Some(false), api_error_text(&e))),
            }
        }
        Message::Edit(id, k, v) => {
            if let Some(f) = state.forms.get_mut(&id) {
                f.revealed.remove(&k);
                f.edits.insert(k, v);
            }
        }
        Message::Bool(id, k, on) => {
            if let Some(f) = state.forms.get_mut(&id) {
                f.bools.insert(k, on);
            }
        }
        Message::Check(id, k, option, on) => {
            if let Some(f) = state.forms.get_mut(&id) {
                let Some(field) = f.fields.iter().find(|x| x.key == k).cloned() else { return Task::none() };
                let mut list = f.checked(&field);
                let key = fields::type_key(&option);
                list.retain(|x| *x != key);
                if on {
                    list.push(key);
                }
                f.checks.insert(k, list);
            }
        }
        Message::Area(id, k, a) => {
            if let Some(f) = state.forms.get_mut(&id) {
                if a.is_edit() {
                    f.typed.insert(k.clone());
                    f.revealed.remove(&k);
                }
                if let Some(c) = f.areas.get_mut(&k) {
                    c.perform(a);
                }
            }
        }
        Message::Reveal(id, k) => {
            if let Some(f) = state.forms.get_mut(&id) {
                if f.edits.get(&k).is_some_and(|v| !v.is_empty()) {
                    f.shown.insert(k);
                    return Task::none();
                }
            }
            let api = ctx.api.clone();
            let path = format!("/admin/settings/secret/{}", crate::pages::urlencode(&k));
            return Task::perform(async move { api.get(&path).await }, move |r| Message::Revealed(id.clone(), k.clone(), r));
        }
        Message::Revealed(id, k, Ok(r)) => {
            if let Some(f) = state.forms.get_mut(&id) {
                let value = fmt::s(&r, "value");
                if let Some(c) = f.areas.get_mut(&k) {
                    *c = text_editor::Content::with_text(&value);
                } else {
                    f.edits.insert(k.clone(), value);
                }
                f.revealed.insert(k.clone());
                f.shown.insert(k);
            }
        }
        Message::Revealed(_, _, Err(e)) => ctx.toast_err(e.message),
        Message::Hide(id, k) => {
            if let Some(f) = state.forms.get_mut(&id) {
                f.shown.remove(&k);
                if f.revealed.remove(&k) {
                    f.edits.remove(&k);
                    if let Some(c) = f.areas.get_mut(&k) {
                        *c = text_editor::Content::new();
                    }
                }
            }
        }
        Message::CopySecret(id, k) => {
            let typed = state.forms.get(&id).and_then(|f| f.edits.get(&k).cloned().or_else(|| f.areas.get(&k).map(|c| c.text()))).unwrap_or_default();
            if !typed.trim().is_empty() {
                ctx.copy(typed);
                ctx.toast_ok("Copied to clipboard");
                return Task::none();
            }
            let api = ctx.api.clone();
            let path = format!("/admin/settings/secret/{}", crate::pages::urlencode(&k));
            return Task::perform(async move { api.get(&path).await }, Message::Copied);
        }
        Message::Copied(Ok(r)) => {
            ctx.copy(fmt::s(&r, "value"));
            ctx.toast_ok("Copied to clipboard");
        }
        Message::Copied(Err(e)) => ctx.toast_err(e.message),
        Message::Clear(id, k) => {
            let builtin = state.forms.get(&id).is_some_and(Form::builtin);
            let mut body = Map::new();
            body.insert(k, Value::String(String::new()));
            let (path, body) = if builtin { ("/admin/settings".to_string(), Value::Object(body)) } else { (plugin_path(&id, "settings"), json!({ "values": body })) };
            let api = ctx.api.clone();
            return Task::perform(async move { api.put(&path, body).await }, move |r| Message::Saved(id.clone(), "clear", r));
        }
        Message::Save(id) => {
            let Some(f) = state.forms.get_mut(&id) else { return Task::none() };
            let (path, body, _) = f.save_call(&id);
            f.saving = true;
            f.flash.clear();
            let api = ctx.api.clone();
            return Task::perform(async move { api.put(&path, body).await }, move |r| Message::Saved(id.clone(), "save", r));
        }
        Message::Saved(id, what, r) => {
            let model_before = vs(&state.settings["translator_model"]);
            if let Some(f) = state.forms.get_mut(&id) {
                f.saving = false;
            }
            match r {
                Ok(v) => {
                    let mut warnings = vec![];
                    if v.get("fields").is_some() {
                        let settings = state.settings.clone();
                        if let Some(f) = state.forms.get_mut(&id) {
                            f.take(v, &settings);
                        }
                    } else {
                        warnings = take_settings(state, v);
                    }
                    if let Some(f) = state.forms.get_mut(&id) {
                        if what == "save" {
                            f.settle();
                        }
                        f.flash = warnings.into_iter().map(|w| (false, w)).collect();
                    }
                    if what == "clear" {
                        ctx.toast_ok("Cleared");
                    } else if id == "translator" && vs(&state.settings["translator_model"]) != model_before {
                        ctx.toast_ok("Saved – Install or Update the Translator to download this model");
                    } else {
                        ctx.toast_ok("Settings saved");
                    }
                    // the status follows the settings (a sign-in, an address)
                    return load_one(ctx, &id);
                }
                Err(e) => ctx.toast_err(api_error_text(&e)),
            }
        }
        Message::Tools(tools::Message::VpnTest) => {
            // the Soulseek card is saved first, then the tunnel is tested with what it holds
            let Some((path, body, _)) = quiet_save(state, "slskd") else {
                return tools::start_vpn_test(&mut state.tools, ctx).map(Message::Tools);
            };
            state.tools.vpn_testing_started();
            let api = ctx.api.clone();
            return Task::perform(async move { api.put(&path, body).await }, Message::VpnSaved);
        }
        Message::VpnSaved(Ok(v)) => {
            saved_quietly(state, "slskd", Some(v));
            return tools::start_vpn_test(&mut state.tools, ctx).map(Message::Tools);
        }
        Message::VpnSaved(Err(e)) => tools::vpn_test_failed(&mut state.tools, e.message),
        Message::Tools(m) => return tools::update(&mut state.tools, m, ctx).map(Message::Tools),
        Message::Go(s) => ctx.navigate(Page::Settings(s)),
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::CopyText(text) => ctx.copy_text(text),
        Message::Tick => {
            let mut tasks = vec![];
            if tools::tidal_waiting(&state.tools) {
                tasks.push(tools::update(&mut state.tools, tools::Message::TidalTick, ctx).map(Message::Tools));
            }
            if anything_busy(state) {
                tasks.push(load_list(ctx));
            }
            return Task::batch(tasks);
        }
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

// ----- views ----------------------------------------------------------------------------------------------------

fn tone_color(tone: &str) -> iced::Color {
    let p = theme::look();
    match tone {
        "ok" => p.ok,
        "err" => p.err,
        "warn" => p.warn,
        _ => p.muted,
    }
}

/// The note under a field: set by an environment variable, a saved secret (with clear), a secret not set, or the
/// environment variable that would set it.
fn source_note<'a>(id: &str, f: &PField, form: &Form) -> Option<El<'a, Message>> {
    let m = &form.values["_meta"][&f.key];
    let set = super::truthy(form.value(&f.key));
    if let Some(note) = env_note(&form.values, &f.key) {
        return Some(help(note).into());
    }
    if f.is_secret() && set {
        return Some(row![help("Saved ✓ (hidden) ·"), link_muted("clear", Message::Clear(id.to_string(), f.key.clone()))].spacing(4).align_y(Alignment::Center).into());
    }
    if f.is_secret() {
        let env = if fmt::truthy(m, "env") { format!(" – or set {} in your compose file", fmt::s(m, "env")) } else { String::new() };
        return Some(help(format!("Not set{env}")).into());
    }
    if m["source"].as_str() == Some("ui") {
        return None;
    }
    if fmt::truthy(m, "env") {
        return Some(widgets::faint(format!("Default · env var {}", fmt::s(m, "env"))).size(12).into());
    }
    None
}

fn field_view<'a>(id: &str, form: &'a Form, f: &'a PField, reveal: bool) -> El<'a, Message> {
    let (pid, key) = (id.to_string(), f.key.clone());
    let locked = form.locked(&f.key);
    let v = form.value(&f.key);
    let reveal = reveal && form.reveals(f);
    let mut lines: Vec<El<'a, Message>> = Vec::new();
    match f.kind.as_str() {
        "bool" => {
            let on = form.bools.get(&f.key).copied().unwrap_or_else(|| super::truthy(v));
            lines.push(switch_maybe(on, f.label.as_str(), (!locked).then_some(move |x| Message::Bool(pid.clone(), key.clone(), x))));
        }
        "checks" => {
            let on = form.checked(f);
            let mut boxes = row![].spacing(14);
            for (val, lab) in &f.options {
                let is = on.contains(&fields::type_key(val));
                let (p, k, o) = (pid.clone(), key.clone(), val.clone());
                boxes = boxes.push(
                    checkbox(is)
                        .label(lab.as_str())
                        .size(16)
                        .text_size(13)
                        .spacing(6)
                        .on_toggle_maybe((!locked).then_some(move |x| Message::Check(p.clone(), k.clone(), o.clone(), x))),
                );
            }
            lines.push(widgets::small_muted(f.label.as_str()).font(theme::semibold()).into());
            lines.push(boxes.wrap().into());
        }
        "select" => {
            let cur = form.current(&f.key);
            let picker: El<'a, Message> = if locked {
                help(f.options.iter().find(|(x, _)| *x == cur).map(|(_, l)| l.clone()).unwrap_or(cur)).into()
            } else {
                select(f.options.iter().map(|(v, l)| Opt::new(v.clone(), l.clone())).collect(), &cur, move |x| Message::Edit(pid.clone(), key.clone(), x))
            };
            lines.push(widgets::field(f.label.as_str(), picker));
        }
        "textarea" => {
            lines.push(widgets::small_muted(f.label.as_str()).font(theme::semibold()).into());
            if let Some(c) = form.areas.get(&f.key) {
                let (p, k) = (pid.clone(), key.clone());
                let placeholder = if f.is_secret() && super::truthy(v) { "Saved – paste to replace" } else { "" };
                lines.push(text_area(c, placeholder, 5, move |a| Message::Area(p.clone(), k.clone(), a)));
            }
            if reveal {
                let shown = form.shown.contains(&f.key);
                lines.push(
                    row![
                        btn_sm(if shown { "Hide" } else { "Show saved" }, Btn::Default, Some(if shown { Message::Hide(pid.clone(), key.clone()) } else { Message::Reveal(pid.clone(), key.clone()) })),
                        btn_icon(Icon::Copy, "Copy", Btn::Default, Some(Message::CopySecret(pid.clone(), key.clone()))),
                    ]
                    .spacing(8)
                    .into(),
                );
            }
        }
        _ if f.is_secret() => {
            let text = form.edits.get(&f.key).cloned().unwrap_or_default();
            let placeholder = if locked {
                "set by environment"
            } else if super::truthy(v) {
                "saved – paste a new value to replace"
            } else {
                "not set"
            };
            let shown = form.shown.contains(&f.key);
            let mut r = row![].spacing(6).align_y(Alignment::Center);
            if locked {
                r = r.push(input_locked::<Message>("").width(Length::Fill));
            } else {
                let (p, k) = (pid.clone(), key.clone());
                r = r.push(input(placeholder, &text, move |x| Message::Edit(p.clone(), k.clone(), x)).secure(!shown).width(Length::Fill));
            }
            if reveal {
                r = r.push(icon_btn(
                    if shown { Icon::X } else { Icon::Search },
                    if shown { "Hide" } else { "Show" },
                    Some(if shown { Message::Hide(pid.clone(), key.clone()) } else { Message::Reveal(pid.clone(), key.clone()) }),
                ));
                r = r.push(icon_btn(Icon::Copy, "Copy", Some(Message::CopySecret(pid.clone(), key.clone()))));
            }
            lines.push(widgets::field(f.label.as_str(), r));
        }
        _ => {
            let text = form.current(&f.key);
            let b: El<'a, Message> = if locked {
                input_locked::<Message>(&text).width(Length::Fill).into()
            } else {
                input("", &text, move |x| Message::Edit(pid.clone(), key.clone(), x)).width(Length::Fill).into()
            };
            lines.push(widgets::field(f.label.as_str(), b));
        }
    }
    if !f.help.is_empty() {
        lines.push(help(f.help.as_str()).into());
    }
    if let Some(n) = source_note(id, f, form) {
        lines.push(n);
    }
    column(lines).spacing(4).width(Length::Fill).into()
}

/// The fields two per row (the settings pages' grid), the spanning ones on a row of their own.
fn grid<'a>(id: &str, form: &'a Form, fields: &[&'a PField], reveal: bool) -> El<'a, Message> {
    let mut rows = column![].spacing(14);
    let mut pending: Option<El<'a, Message>> = None;
    for f in fields {
        let el = field_view(id, form, f, reveal);
        if f.spans() {
            if let Some(p) = pending.take() {
                rows = rows.push(super::two(p, None));
            }
            rows = rows.push(el);
        } else if let Some(p) = pending.take() {
            rows = rows.push(super::two(p, Some(el)));
        } else {
            pending = Some(el);
        }
    }
    if let Some(p) = pending.take() {
        rows = rows.push(super::two(p, None));
    }
    rows.width(Length::Fill).into()
}

/// The fields of a card in runs under their `part` heading, in order.
pub fn runs<'a>(fields: &[&'a PField]) -> Vec<(String, Vec<&'a PField>)> {
    let mut out: Vec<(String, Vec<&'a PField>)> = vec![];
    for f in fields {
        match out.last_mut() {
            Some((p, list)) if *p == f.part => list.push(f),
            _ => out.push((f.part.clone(), vec![*f])),
        }
    }
    out
}

/// What a part heading says: a part that was a card of its own before 0.32 (YouTube sign-in) keeps that card's
/// description and Test button. (desc, (test, its words), the card it was)
fn part_info(part: &str) -> (&'static str, Option<(&'static str, &'static str)>, &'static str) {
    match fields::SECTIONS.iter().find(|c| c.title == part) {
        Some(c) => (c.desc, c.test.map(|t| (t, c.test_label.unwrap_or("Test connection"))), c.id),
        None => ("", None, ""),
    }
}

/// A download client's note, from its fields' `when` rule (the select of the Torrents & Usenet card).
fn client_note_of<'a>(c: &Value, answer: &Value, settings: &Value) -> Option<El<'a, Message>> {
    let rule = fmt::items(&answer["fields"]).iter().filter_map(|f| f["when"].as_str().and_then(parse_when)).find(|w| w.2)?;
    let (select_label, option) = match option_label(&rule.0, &rule.1) {
        Some((l, o)) => (l.to_string(), o),
        None => (rule.0.clone(), rule.1.clone()),
    };
    let chosen = vs(&settings[&rule.0]);
    let chosen_words = option_label(&rule.0, &chosen).map(|(_, w)| w).unwrap_or_else(|| chosen.clone());
    let (tone, words) = client_note(&fmt::s(c, "name"), &select_label, &option, chosen == rule.1, &chosen_words);
    Some(t(words).size(12.5).color(tone_color(tone)).into())
}

/// The Torrents & Usenet card: each chosen client has a plugin of its own, which has to be installed and on.
fn chosen_clients<'a>(state: &State) -> Vec<El<'a, Message>> {
    let mut out = vec![];
    for key in ["torrent_client", "nzb_client"] {
        let v = vs(&state.settings[key]);
        let Some((_, _, id)) = CLIENTS.iter().find(|(k, val, _)| *k == key && *val == v) else { continue };
        let c = card_of(state, id);
        if !fmt::truthy(&c, "name") {
            continue;
        }
        let (label, chosen) = option_label(key, &v).unwrap_or((key, v.clone()));
        let Some((line, act)) = chosen_client_line(label, &chosen, &c) else { continue };
        let free = !state.acting.contains_key(*id);
        let button: El<'a, Message> = if act == "install" {
            btn_icon(Icon::Download, "Install", Btn::Default, free.then(|| Message::Act(id.to_string(), "install"))).into()
        } else {
            btn_sm("Enable", Btn::Default, free.then(|| Message::Act(id.to_string(), "enable"))).into()
        };
        out.push(row![t(line).size(12.5).color(theme::look().warn), button].spacing(8).align_y(Alignment::Center).wrap().into());
    }
    out
}

/// A plugin's settings: its fields under their headings, the blocks of the source cards that moved here
/// (streamrip's Tidal sign-in and config file, Soulseek's VPN and status, yt-dlp's YouTube sign-in) and Save.
fn form_view<'a>(state: &'a State, c: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let id = c["id"].as_str().unwrap_or_default();
    let Some(form) = state.forms.get(id) else {
        return widgets::skeleton_rows(3);
    };
    if !form.error.is_empty() {
        return result_line_copy(false, &form.error, Message::CopyText);
    }
    let reveal = ctx.has_feature("reveal") && ctx.session.is_owner();
    let mut body = column![].spacing(14).width(Length::Fill);
    if fmt::s(c, "kind") == "client" {
        if let Some(n) = client_note_of(c, &form.answer, &state.settings) {
            body = body.push(n);
        }
    }
    let shown: Vec<&PField> = form.fields.iter().filter(|f| form.visible(f)).collect();
    for (part, fields) in runs(&shown) {
        if !part.is_empty() {
            let (desc, test, card) = part_info(&part);
            let mut words = column![widgets::h3(part.clone())].spacing(4).width(Length::Fill);
            if !desc.is_empty() {
                words = words.push(help(desc));
            }
            let mut head = row![words].spacing(10).align_y(Alignment::Start);
            if let Some((test, label)) = test {
                let testing = state.part_testing.contains(id);
                head = head.push(btn_icon(Icon::Refresh, if testing { "Testing…" } else { label }, Btn::Default, (!testing).then(|| Message::PartTest(id.to_string(), test))));
            }
            body = body.push(head);
            if let Some((ok, m)) = state.part_outs.get(id) {
                body = body.push(if state.part_testing.contains(id) { help(m.as_str()).into() } else { result_line_copy(*ok, m, Message::CopyText) });
            }
            if card == "youtube" {
                body = body.push(tools::youtube_block(&state.tools).map(Message::Tools));
            }
        }
        if id == "slskd" {
            let (vpn, rest): (Vec<&PField>, Vec<&PField>) = fields.iter().copied().partition(|f| f.key.starts_with("vpn_"));
            body = body.push(grid(id, form, &rest, reveal));
            if !vpn.is_empty() {
                body = body.push(tools::vpn_block(
                    &state.tools,
                    grid(id, form, &vpn, reveal),
                    (!state.tools.vpn_testing()).then_some(Message::Tools(tools::Message::VpnTest)),
                    Message::CopyText,
                ));
            }
        } else {
            body = body.push(grid(id, form, &fields, reveal));
        }
    }
    if form.builtin() {
        match id {
            "streamrip" => {
                body = body.push(tools::tidal_block(&state.tools).map(Message::Tools));
                body = body.push(tools::streamrip_block(&state.tools).map(Message::Tools));
            }
            "slskd" => body = body.push(tools::soulseek_block(&state.tools).map(Message::Tools)),
            "torznab" => {
                for l in chosen_clients(state) {
                    body = body.push(l);
                }
            }
            _ => {}
        }
    }
    for (ok, m) in &form.flash {
        body = body.push(result_line_copy(*ok, m, Message::CopyText));
    }
    body = body.push(
        container(btn_icon(Icon::Check, if form.saving { "Saving…" } else { "Save" }, Btn::Primary, (!form.saving).then(|| Message::Save(id.to_string()))))
            .width(Length::Fill)
            .align_x(Alignment::End),
    );
    body.into()
}

/// The badges of a card or row: kind, third-party, off, an update (or up to date).
fn badges<'a>(c: &Value) -> El<'a, Message> {
    let mut r = row![badge(kind_label(&fmt::s(c, "kind")), Badge::Plain)].spacing(6).align_y(Alignment::Center);
    if fmt::s(c, "trust") == "third-party" || (fmt::truthy(c, "origin") && fmt::s(c, "origin") != "catalog") {
        r = r.push(badge("Third-party", Badge::Warn));
    }
    if fmt::truthy(c, "installed") && !fmt::truthy(c, "enabled") {
        r = r.push(badge("Off", Badge::Plain));
    }
    if fmt::truthy(c, "installed") && fmt::truthy(c, "update_available") && fmt::truthy(c, "latest") {
        r = r.push(badge(format!("{} available", fmt::s(c, "latest")), Badge::Info));
    } else if fmt::truthy(c, "installed") && fmt::truthy(c, "latest") {
        r = r.push(badge("✓ Up to date", Badge::Ok));
    }
    r.wrap().into()
}

/// The name line: the dot (installed plugins), the name, the version.
fn name_line<'a>(c: &Value, size: f32) -> El<'a, Message> {
    let mut r = row![].spacing(8).align_y(Alignment::Center);
    if fmt::truthy(c, "installed") {
        let color = match dot_tone(c) {
            "" => theme::look().faint,
            tone => tone_color(tone),
        };
        r = r.push(t("●").size(11).color(color));
    }
    r = r.push(t(fmt::s(c, "name")).size(size).font(theme::bold()));
    r = r.push(widgets::small_muted(version_text(c)).font(theme::mono()));
    r.into()
}

/// The status line of a card or row, with the last error under it.
fn status_view<'a>(state: &State, c: &Value) -> Option<El<'a, Message>> {
    let id = fmt::s(c, "id");
    let (tone, words) = status_words(c, state.acting.get(&id).map(String::as_str))?;
    let mut col = column![].spacing(4);
    col = col.push(if tone == "err" { result_line_copy(false, &words, Message::CopyText) } else { t(words).size(13).color(tone_color(tone)).into() });
    if fmt::truthy(c, "last_error") && !state.acting.contains_key(&id) && !fmt::truthy(c, "busy") {
        col = col.push(result_line_copy(false, &fmt::s(c, "last_error"), Message::CopyText));
    }
    Some(col.into())
}

/// The about line with the homepage link.
fn about_view<'a>(c: &Value) -> Option<El<'a, Message>> {
    let words = about_words(c);
    let home = fmt::s(c, "homepage");
    if words.is_empty() && !safe_link(&home) {
        return None;
    }
    let mut r = row![].spacing(6).align_y(Alignment::Center);
    if !words.is_empty() {
        r = r.push(help(words));
    }
    if safe_link(&home) {
        r = r.push(link("Homepage", Message::OpenUrl(home.trim().to_string())));
    }
    Some(r.wrap().into())
}

/// The buttons of an installed plugin's card, and its auto-update switch.
fn actions_view<'a>(state: &State, c: &Value) -> El<'a, Message> {
    let id = fmt::s(c, "id");
    let acting = state.acting.get(&id).map(String::as_str);
    let busy = acting.is_some() || fmt::truthy(c, "busy");
    let b = |act: &'static str, kind: Btn, ic: Option<Icon>, words: String| -> El<'a, Message> {
        if acting == Some(act) {
            return btn_sm(busy_words(act), Btn::Default, None::<Message>).into();
        }
        let on = (!busy).then(|| if act == "test" { Message::Test(id.clone()) } else { Message::Act(id.clone(), act) });
        match ic {
            Some(i) => btn_icon(i, words, kind, on).into(),
            None => btn_sm(words, kind, on).into(),
        }
    };
    let mut r = row![].spacing(8).align_y(Alignment::Center);
    if fmt::truthy(c, "can_update") {
        if fmt::truthy(c, "update_available") && fmt::truthy(c, "latest") {
            r = r.push(b("update", Btn::Primary, Some(Icon::Download), format!("Update to {}", fmt::s(c, "latest"))));
        } else {
            r = r.push(b("update", Btn::Default, Some(Icon::Download), "Update".into()));
        }
    }
    r = r.push(b("test", Btn::Default, Some(Icon::Refresh), test_label(&id).into()));
    // the logs drawer opens and closes while the plugin works too
    r = r.push(btn_icon(Icon::List, "Logs", Btn::Default, Some(Message::Logs(id.clone()))));
    if fmt::truthy(c, "enabled") {
        r = r.push(b("disable", Btn::Default, None, "Disable".into()));
    } else {
        r = r.push(b("enable", Btn::Primary, Some(Icon::Check), "Enable".into()));
    }
    r = r.push(b("remove", Btn::Danger, Some(Icon::Trash), "Remove".into()));
    let mut col = column![r.wrap()].spacing(10);
    if fmt::truthy(c, "can_update") {
        let i2 = id.clone();
        col = col.push(if busy {
            switch_maybe(fmt::truthy(c, "auto_update"), auto_words(&id), None::<fn(bool) -> Message>)
        } else {
            switch(fmt::truthy(c, "auto_update"), auto_words(&id), move |on| Message::Auto(i2.clone(), on))
        });
    }
    col.into()
}

/// The logs drawer of a card.
fn logs_view<'a>(id: &str, l: &'a Logs) -> El<'a, Message> {
    let i = id.to_string();
    let head = super::between(
        t("Log").size(14).font(theme::bold()).into(),
        row![
            btn_icon(Icon::Refresh, "Refresh", Btn::Default, (!l.loading).then(|| Message::LogsRefresh(i.clone()))),
            btn_icon(Icon::Copy, "Copy", Btn::Default, (!l.text.is_empty()).then(|| Message::LogCopy(i.clone()))),
        ]
        .spacing(6)
        .into(),
    );
    let mut c = column![head].spacing(8);
    if !l.file.is_empty() {
        c = c.push(widgets::faint(l.file.as_str()).size(12).font(theme::mono()));
    }
    if l.loading && l.text.is_empty() {
        c = c.push(help("Loading…"));
    } else {
        let i2 = i.clone();
        c = c.push(
            text_editor::TextEditor::new(&l.content)
                .on_action(move |a| Message::LogText(i2.clone(), a))
                .font(theme::mono())
                .size(12)
                .padding(10)
                .height(Length::Fixed(320.0))
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                .style(crate::widgets::text_area::style),
        );
    }
    container(c).padding([12, 14]).width(Length::Fill).style(theme::card).into()
}

/// An installed plugin's card: a header that folds it (the dot, name, version, badges, description), then the
/// warning, the status, the about line, the buttons, the last answer, the logs drawer and the settings.
fn installed_card<'a>(state: &'a State, c: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let id = fmt::s(c, "id");
    let open = state.open.contains(&id);
    let mut words = column![name_line(c, 19.0), badges(c)].spacing(6).width(Length::Fill);
    if fmt::truthy(c, "description") {
        words = words.push(help(fmt::s(c, "description")));
    }
    let header = button(row![words, container(icon(if open { Icon::Chevron } else { Icon::More }, 18.0, p.muted)).padding([4, 0])].spacing(10).align_y(Alignment::Start))
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press(Message::Toggle(id.clone()));
    let mut body = column![header].spacing(12).width(Length::Fill);
    if !open {
        return widgets::card(body).width(Length::Fill).into();
    }
    if fmt::truthy(c, "warning") {
        body = body.push(container(t(fmt::s(c, "warning")).size(13)).padding([10, 14]).width(Length::Fill).style(theme::warn_box));
    }
    if let Some(s) = status_view(state, c) {
        body = body.push(s);
    }
    if let Some(a) = about_view(c) {
        body = body.push(a);
    }
    body = body.push(actions_view(state, c));
    if let Some((ok, m)) = state.outs.get(&id) {
        body = body.push(if state.acting.get(&id).is_some_and(|a| a == "test") { help(m.as_str()).into() } else { result_line_copy(*ok, m, Message::CopyText) });
    }
    if let Some(l) = state.logs.get(&id) {
        body = body.push(logs_view(&id, l));
    }
    // its settings: here, or on the page that has them
    let on = fmt::s(c, "settings_on");
    if !on.is_empty() {
        if let Some(s) = Section::by_id(&on).filter(|s| *s != Section::Plugins) {
            body = body.push(row![help("Its settings are on"), link(format!("Settings → {}", s.label()), Message::Go(s))].spacing(4).align_y(Alignment::Center).wrap());
        }
    } else if has_form(c) {
        body = body.push(widgets::divider());
        body = body.push(form_view(state, c, ctx));
    }
    widgets::card(body).width(Length::Fill).into()
}

/// A plugin that is not installed: a row of the Available list.
fn available_row<'a>(state: &'a State, c: &'a Value) -> El<'a, Message> {
    let id = fmt::s(c, "id");
    let mut left = column![name_line(c, 14.0), badges(c)].spacing(4).width(Length::Fill);
    if fmt::truthy(c, "description") {
        left = left.push(help(fmt::s(c, "description")));
    }
    if let Some(a) = about_view(c) {
        left = left.push(a);
    }
    if let Some(s) = status_view(state, c) {
        left = left.push(s);
    }
    if let Some((ok, m)) = state.outs.get(&id) {
        left = left.push(result_line_copy(*ok, m, Message::CopyText));
    }
    let busy = state.acting.contains_key(&id) || fmt::truthy(c, "busy");
    let action: El<'a, Message> = if state.acting.get(&id).is_some_and(|a| a == "install") {
        btn_sm(busy_words("install"), Btn::Default, None::<Message>).into()
    } else {
        btn_icon(Icon::Download, "Install", Btn::Primary, (!busy).then(|| Message::Act(id.clone(), "install"))).into()
    };
    column![container(row![left, action].spacing(12).align_y(Alignment::Start)).padding([12, 14]), widgets::divider()].into()
}

/// The sandbox line at the top of the page (`sandbox` of GET /api/plugins).
fn sandbox_view<'a>(sb: &Value) -> El<'a, Message> {
    let full = fmt::s(sb, "level") == "full";
    let text = if fmt::truthy(sb, "text") {
        fmt::s(sb, "text")
    } else if full {
        "Full isolation".into()
    } else {
        "Limits only, see the compose option".into()
    };
    let what = if full {
        "Plugins you add see only their own folders, and the network only when they ask for it."
    } else {
        "Plugins you add run as Encore's user with limits on memory, processor time and processes; the compose option in the README gives them full isolation."
    };
    let mut c = column![row![
        icon(Icon::Shield, 15.0, theme::look().muted),
        t("Sandbox").size(13).font(theme::bold()),
        badge(text, if full { Badge::Ok } else { Badge::Warn }),
        help(what),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .wrap()]
    .spacing(4);
    if fmt::truthy(sb, "reason") {
        c = c.push(widgets::faint(fmt::s(sb, "reason")).size(12));
    }
    c.into()
}

/// The page's first card: Check for updates and the sandbox line.
fn head_view<'a>(state: &'a State, list: &'a Value) -> El<'a, Message> {
    widgets::card(
        column![
            super::between(
                widgets::h2("Plugins").into(),
                btn_icon(Icon::Refresh, if state.checking { "Checking…" } else { "Check for updates" }, Btn::Default, (!state.checking).then_some(Message::CheckAll)).into(),
            ),
            row![
                help("Download sources and clients, server tools and connections to other music servers. Each one installs, updates and is removed on its own and is kept in the config folder, so no new Encore image is needed. Installed plugins are checked for updates daily; change the schedule under"),
                link("Scheduled tasks", Message::Go(Section::Tasks)),
            ]
            .spacing(2)
            .align_y(Alignment::Center)
            .wrap(),
            sandbox_view(&list["sandbox"]),
        ]
        .spacing(12)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .into()
}

/// Upload plugin and Install from URL or GitHub.
fn add_view<'a>(state: &'a State, list: &'a Value) -> El<'a, Message> {
    let max = if fmt::i(list, "upload_max_mb") > 0 { fmt::i(list, "upload_max_mb") } else { 200 };
    let mut c = column![
        widgets::h3("Add a plugin"),
        help("A plugin is a zip with a plugin.toml and its program, for example one made with Encore's plugin SDK (docs/plugins in Encore's repository). Plugins you add are not made by Encore: install only plugins you trust."),
        row![
            btn_icon(Icon::Plus, "Upload plugin", Btn::Default, (!state.adding).then_some(Message::PickUpload)),
            help(format!("A .zip of at most {max} MB (or drop it here)")),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            input("https://… or owner/repo@ref", &state.url, Message::Url).on_submit(Message::InstallUrl).width(Length::Fill),
            btn_icon(Icon::Link, "Install from URL", Btn::Default, (!state.adding).then_some(Message::InstallUrl)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        help("The address of a plugin zip or of its plugin.toml, or a GitHub repository as owner/repo@ref (a branch, tag or commit; owner/repo/folder@ref for a plugin in a folder)."),
    ]
    .spacing(10)
    .width(Length::Fill);
    match &state.add_out {
        Some((None, m)) => c = c.push(help(m.as_str())),
        Some((Some(ok), m)) => c = c.push(result_line_copy(*ok, m, Message::CopyText)),
        None => {}
    }
    drop_zone::view(widgets::card(c).width(Length::Fill), &state.drop, "Drop a plugin .zip to upload it")
}

/// The plugin zips *Upload plugin* takes, picked or dropped.
const PLUGIN_FILES: drop_zone::Accept = drop_zone::Accept::one(&["zip"]);

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(list) = &state.list else {
        return widgets::skeleton_rows(6);
    };
    if state.missing {
        return super::card(
            "Plugins",
            vec![
                help("This server is older than Encore 0.32 and has no plugins: its download sources are under Settings → Downloads, and its download tools on the server's website (Settings → Downloads → Download tools).").into(),
                link("Open Downloads", Message::Go(Section::Downloads)),
            ],
        );
    }
    let label = |s: &'a str| -> El<'a, Message> { container(t(s).size(11).color(ctx.look.faint).font(theme::bold())).padding(widgets::pad(10.0, 4.0, 0.0, 4.0)).into() };
    let all = cards(state);
    let mut page = column![head_view(state, list), label("Installed")].spacing(16).width(Length::Fill);
    let installed: Vec<&Value> = all.iter().copied().filter(|c| fmt::truthy(c, "installed")).collect();
    if installed.is_empty() {
        page = page.push(widgets::card(help("No plugins installed yet. Install the ones you want to use from the list below.")).width(Length::Fill));
    }
    for c in installed {
        page = page.push(installed_card(state, c, ctx));
    }
    let available: Vec<&Value> = all.iter().copied().filter(|c| !fmt::truthy(c, "installed")).collect();
    if !available.is_empty() {
        page = page.push(label("Available"));
        let mut rows = column![];
        for c in available {
            rows = rows.push(available_row(state, c));
        }
        page = page.push(widgets::card_flat(rows).width(Length::Fill));
    }
    page.push(add_view(state, list)).into()
}

pub fn overlay<'a>(state: &'a State) -> Option<El<'a, Message>> {
    match &state.dialog {
        Some(Dialog::Remove { title, text, .. }) => return Some(widgets::confirm(title, text, "Remove", true, Message::Remove, Message::CloseDialog)),
        Some(Dialog::Trust(add)) => {
            let full = state.list.as_ref().is_some_and(|l| fmt::s(&l["sandbox"], "level") == "full");
            return Some(widgets::confirm("Install a plugin you add", trust_text(full), "Install", false, Message::Install(add.clone()), Message::CloseDialog));
        }
        None => {}
    }
    tools::overlay(&state.tools).map(|d| d.map(Message::Tools))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    let mut subs = vec![];
    // the Soulseek and VPN states while the Soulseek card is open
    if state.open.contains("slskd") {
        subs.push(iced::time::every(Duration::from_secs(5)).map(|_| Message::Tools(tools::Message::Tick)));
    }
    if tools::tidal_waiting(&state.tools) || anything_busy(state) {
        subs.push(iced::time::every(Duration::from_secs(3)).map(|_| Message::Tick));
    }
    // plugin zips dragged onto the window (Upload plugin)
    subs.push(drop_zone::events().map(Message::Drop));
    Subscription::batch(subs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(fields: Value, values: Value, settings: &Value) -> Form {
        Form::new(json!({ "id": "x", "builtin": true, "fields": fields, "values": values }), settings)
    }

    /// A card of GET /api/plugins.
    fn card(id: &str, installed: bool) -> Value {
        json!({ "id": id, "name": id, "description": format!("What {id} does"), "kind": "source", "version": "0.32.0",
            "installed": installed, "installed_version": if installed { json!("1.2.3") } else { Value::Null }, "enabled": installed,
            "origin": "catalog", "trust": "first-party", "builtin": true, "warning": null, "auto_update": true, "can_update": true,
            "latest": null, "update_available": false, "last_error": null, "status": null, "busy": null, "sandbox": "Full isolation",
            "network": true, "hosts": [], "reads_library": false, "kinds": ["track", "album"], "lossless": false,
            "settings_on": null, "has_settings": true, "homepage": "https://example.org", "license": "MIT" })
    }

    #[test]
    fn the_moved_cards_and_old_links_open_the_plugins_page() {
        for s in ["streamrip", "torznab", "slskd", "ytdlp", "youtube", "tools", "plugins", "qbittorrent", "translator"] {
            assert!(opens_plugins(s), "{s}");
        }
        for s in ["pipeline", "releases", "apps", "downloads", "cadenza", "media", "account"] {
            assert!(!opens_plugins(s), "{s}");
        }
        assert_eq!(card_for("youtube"), Some("ytdlp"));
        assert_eq!(card_for("slskd"), Some("slskd"));
        assert_eq!(card_for("qbittorrent"), Some("qbittorrent"));
        assert_eq!(card_for("tools"), None);
        assert_eq!(card_for("plugins"), None);
        assert_eq!(plugin_path("torrent-blackhole", "install"), "/plugins/torrent-blackhole/install");
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn a_builtin_card_saves_encores_settings_like_the_settings_pages() {
        // GET /admin/settings: the values with `_meta`
        let settings = json!({ "streamrip_enabled": true, "streamrip_quality": 4, "streamrip_deezer_arl": "•••• (saved)", "streamrip_bin": "/x",
            "_meta": { "streamrip_bin": { "source": "env", "env": "STREAMRIP_BIN" }, "streamrip_quality": { "source": "ui" } } });
        let f = builtin(
            json!([
                { "key": "streamrip_enabled", "label": "Enable streamrip source", "type": "bool" },
                { "key": "streamrip_quality", "label": "Max quality", "type": "select", "options": [{ "value": "2", "label": "CD" }, { "value": "4", "label": "Hi-res max" }] },
                { "key": "streamrip_deezer_arl", "label": "Deezer ARL cookie", "type": "password", "secret": true },
                { "key": "streamrip_bin", "label": "streamrip executable (optional)", "type": "text" },
                { "key": "ytdlp_cookies", "label": "Pasted cookies.txt (backup)", "type": "textarea", "secret": true, "part": "YouTube sign-in" },
                { "key": "youtube_light_refresh", "label": "Refresh pasted cookies every 4 hours", "type": "bool" }
            ]),
            json!({ "ytdlp_cookies": "•••• (saved)", "youtube_light_refresh": false }),
            &settings,
        );
        let (path, body, encore) = f.save_call("streamrip");
        assert_eq!((path.as_str(), encore), ("/admin/settings", true), "Encore's own keys go where they always went");
        assert_eq!(body["streamrip_enabled"], true);
        assert_eq!(body["streamrip_quality"], "4", "a number shows and goes back as the select's text");
        assert!(body.get("streamrip_deezer_arl").is_none(), "nothing typed keeps the saved secret");
        assert!(body.get("streamrip_bin").is_none(), "set by the environment");
        assert!(body.get("ytdlp_cookies").is_none(), "a secret text area starts empty and stays out");
        assert_eq!(body["youtube_light_refresh"], false, "a key /admin/settings lacks is taken from the plugin's answer");
        // the part runs on: the field after a part's first one is in it too
        assert_eq!(f.fields[5].part, "YouTube sign-in");
        let shown: Vec<&PField> = f.fields.iter().collect();
        let r = runs(&shown);
        assert_eq!(r.iter().map(|(p, fs)| (p.as_str(), fs.len())).collect::<Vec<_>>(), [("", 4), ("YouTube sign-in", 2)]);
        let mut f = f;
        f.edits.insert("streamrip_deezer_arl".into(), " arl ".into());
        f.bools.insert("streamrip_enabled".into(), false);
        let b = f.save_body();
        assert_eq!(b["streamrip_deezer_arl"], "arl");
        assert_eq!(b["streamrip_enabled"], false);
        // a secret shown from the server is not sent back as if it were typed
        f.revealed.insert("streamrip_deezer_arl".into());
        assert!(f.save_body().get("streamrip_deezer_arl").is_none());
        f.settle();
        assert!(f.edits.is_empty() && f.bools.is_empty());
        assert!(f.reveals(&f.fields[2]) && !f.reveals(&f.fields[0]));
    }

    #[test]
    fn a_plugins_own_card_saves_to_the_plugin() {
        let f = Form::new(
            json!({ "id": "internet-archive", "builtin": false, "fields": [
                { "key": "format", "label": "Format", "type": "select", "options": [{ "value": "flac", "label": "FLAC" }, { "value": "mp3", "label": "MP3" }] },
                { "key": "token", "label": "Token", "type": "password" },
                { "key": "kinds", "label": "Kinds", "type": "checks", "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] } ],
                "values": { "format": "flac", "token": "•••• (saved)", "kinds": "b" } }),
            &json!({ "format": "ignored", "_meta": {} }),
        );
        let (path, body, encore) = f.save_call("internet-archive");
        assert_eq!((path.as_str(), encore), ("/plugins/internet-archive/settings", false));
        assert_eq!(body, json!({ "values": { "format": "flac", "kinds": "b" } }), "Encore's settings are not this plugin's");
        assert!(!f.reveals(&f.fields[1]), "the eye asks Encore's settings: not for a plugin's own");
        assert!(!f.locked("format"));
    }

    #[test]
    fn when_rules_only_hide_by_a_select_on_the_same_card() {
        // a download client's card: its fields name the torrent client chosen on Torrents & Usenet
        let f = builtin(json!([{ "key": "qbit_url", "label": "qBittorrent URL", "when": "torrent_client=qbittorrent" }]), json!({ "qbit_url": "http://q" }), &Value::Null);
        assert!(f.visible(&f.fields[0]), "the client card is shown whole");
        let mut f = builtin(
            json!([
                { "key": "mode", "label": "Mode", "type": "select", "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] },
                { "key": "a_only", "label": "A only", "when": "mode=a" },
                { "key": "not_a", "label": "Not A", "when": "mode!=a" }
            ]),
            json!({ "mode": "a" }),
            &Value::Null,
        );
        assert!(f.visible(&f.fields[1]) && !f.visible(&f.fields[2]));
        f.edits.insert("mode".into(), "b".into());
        assert!(!f.visible(&f.fields[1]) && f.visible(&f.fields[2]));
        assert_eq!(parse_when("bad"), None);
    }

    #[test]
    fn words_match_the_web() {
        assert_eq!(kind_label("client"), "Download client");
        assert_eq!(kind_label("weird"), "Weird");
        assert_eq!(kinds_words(&[json!("track"), json!("album"), json!("url")]), "songs, albums and links");
        let before = card("ytdlp", true);
        let mut after = before.clone();
        assert_eq!(done_words("update", &before, &after), "ytdlp is up to date");
        // the newest release needs a Python the server cannot get: it stayed, and the card's note says why
        after["last_error"] = json!("Staying on ytdlp …");
        assert_eq!(done_words("update", &before, &after), format!("ytdlp stays on {}", version_text(&after)));
        after["last_error"] = Value::Null;
        after["installed_version"] = json!("2.0");
        assert_eq!(done_words("update", &before, &after), "ytdlp updated to 2.0");
        assert_eq!(done_words("install", &card("plex", false), &json!({ "name": "Plex", "installed": true })), "Plex installed");
        assert_eq!(done_words("disable", &before, &after), "ytdlp disabled");
        assert_eq!(check_words(&json!({ "plugins": [] })), "No updates found");
        assert_eq!(check_words(&json!({ "plugins": [{ "installed": true, "update_available": true }, { "installed": false, "update_available": true }] })), "1 update available");
        assert_eq!(error_text("plugin.toml has problems", &json!({ "problems": ["[plugin] id: missing", "[run] shell: not allowed"] })), "plugin.toml has problems\n- [plugin] id: missing\n- [run] shell: not allowed");
        assert_eq!(error_text("", &Value::Null), "Failed");
        assert_eq!(test_answer(&json!({ "ok": false, "error": "Too many requests", "code": "rate_limited", "retry_after": 90 })).1, "Too many requests – try again in 1 m 30 s");
        assert_eq!(test_answer(&json!({ "ok": true })), (true, "Test passed".to_string()));
        assert_eq!(url_problem(" "), Some("Enter an https address, or owner/repo@ref for GitHub"));
        assert_eq!(url_problem("http://x/p.zip"), Some("Plugins install only from https addresses"));
        assert_eq!(url_problem("Krifmaz/encore-plugins/internet-archive@v0.1.0"), None);
        assert_eq!(file_problem("plugin.tar.gz", 10.0, 200).as_deref(), Some("A plugin is uploaded as a .zip file"));
        assert_eq!(file_problem("p.zip", 300.0 * 1048576.0, 200).as_deref(), Some("The plugin zip is larger than 200 MB"));
        assert_eq!(remove_words("yt-dlp", true).1, "Remove yt-dlp? Encore stops using it until you install it again. What the plugin downloaded is deleted; your settings for it are kept.");
        assert_eq!(remove_words("Internet Archive", false).1, "Remove Internet Archive? Its program, data, log and settings are deleted.");
        assert_eq!(test_label("streamrip"), "Test & write config");
        assert_eq!(test_label("slskd"), "Test connection");
        assert_eq!(test_label("ytdlp"), "Test");
        assert_eq!(test_label("internet-archive"), "Test");
        assert!(auto_words("ytdlp").contains("sites change often"));
        let (desc, test, was) = part_info("YouTube sign-in");
        assert!(desc.starts_with("Signs yt-dlp in to YouTube"));
        assert_eq!(test, Some(("youtube", "Test cookies")));
        assert_eq!(was, "youtube");
    }

    #[test]
    fn status_and_about_lines() {
        let mut c = card("ytdlp", true);
        assert_eq!(status_words(&c, None), Some(("", "Checking…".to_string())));
        assert_eq!(status_words(&c, Some("update")), Some(("", "Updating…".to_string())));
        c["status"] = json!({ "ok": true, "message": "yt-dlp 2026.10.14" });
        assert_eq!(status_words(&c, None), Some(("ok", "yt-dlp 2026.10.14".to_string())));
        assert_eq!(dot_tone(&c), "ok");
        c["status"] = json!({ "ok": true, "signed_in": false, "message": "Jellyfin needs its address (Settings → Integrations)" });
        assert_eq!(dot_tone(&c), "warn", "works, but needs setting up");
        c["status"] = json!({ "ok": true, "message": "yt-dlp 2026.10.14" });
        c["enabled"] = json!(false);
        assert_eq!(status_words(&c, None).unwrap().1, "Disabled – Encore does not use it until you enable it again");
        assert_eq!(dot_tone(&c), "");
        assert_eq!(status_words(&card("plex", false), None), None);
        assert_eq!(about_words(&card("ytdlp", true)), "Finds songs and albums · Licence MIT");
        let mut up = card("internet-archive", true);
        up["trust"] = json!("third-party");
        up["origin"] = json!("upload");
        up["hosts"] = json!(["archive.org"]);
        up["lossless"] = json!(true);
        assert_eq!(about_words(&up), "Finds songs and albums, lossless · Sandbox: Full isolation · Network: archive.org · Licence MIT");
        up["network"] = json!(false);
        up["reads_library"] = json!(true);
        assert!(about_words(&up).contains("No network · Reads your library (read only)"));
        assert!(!safe_link("javascript:alert(1)") && safe_link("https://archive.org"));
    }

    #[test]
    fn download_clients_say_whether_they_are_in_use() {
        assert_eq!(
            client_note("qBittorrent", "Torrent client", "qBittorrent", true, "qBittorrent"),
            ("", "In use: Torrent client is set to qBittorrent on the Torrents & Usenet card.".to_string())
        );
        assert_eq!(
            client_note("Deluge", "Torrent client", "Deluge", false, "qBittorrent"),
            ("warn", "Not in use: Torrent client is set to qBittorrent on the Torrents & Usenet card. Choose Deluge there to send downloads to Deluge.".to_string())
        );
        assert_eq!(option_label("torrent_client", "rtorrent"), Some(("Torrent client", "rTorrent / ruTorrent".to_string())));
        let c = card("qbittorrent", false);
        assert_eq!(chosen_client_line("Torrent client", "qBittorrent", &c), Some(("Torrent client: qBittorrent has a plugin of its own, not installed yet.".to_string(), "install")));
        let mut c = card("qbittorrent", true);
        c["enabled"] = json!(false);
        assert_eq!(chosen_client_line("Torrent client", "qBittorrent", &c).unwrap().1, "enable");
        assert_eq!(chosen_client_line("Torrent client", "qBittorrent", &card("qbittorrent", true)), None);
    }

    #[test]
    fn the_page_follows_the_server() {
        super::super::with_test_ctx(|ctx| {
            let mut st = State::default();
            // an older server answers /api/plugins with its web page
            let _ = update(&mut st, Message::List(Ok(Value::String("<!doctype html>".into()))), ctx);
            assert!(st.missing);
            let mut st = State::default();
            let mut cadenza = card("cadenza", true);
            cadenza["settings_on"] = json!("processing");
            let list = json!({ "sandbox": { "level": "full", "text": "Full isolation" }, "upload_max_mb": 200,
                "plugins": [card("ytdlp", true), cadenza, card("plex", false)] });
            let _ = update(&mut st, Message::List(Ok(list)), ctx);
            assert!(!st.missing);
            assert!(has_form(&card_of(&st, "ytdlp")));
            assert!(!has_form(&card_of(&st, "cadenza")), "Cadenza's fields stay on Server processing");
            // a status answer replaces the card
            let mut yt = card("ytdlp", true);
            yt["status"] = json!({ "ok": true, "message": "yt-dlp 2026.10.14" });
            let _ = update(&mut st, Message::One("ytdlp".into(), Ok(yt)), ctx);
            assert_eq!(card_of(&st, "ytdlp")["status"]["message"], "yt-dlp 2026.10.14");
            // an action is sent once until it answers
            let _ = update(&mut st, Message::Act("plex".into(), "install"), ctx);
            assert_eq!(st.acting.get("plex").map(String::as_str), Some("install"));
            let _ = update(&mut st, Message::Act("plex".into(), "install"), ctx);
            let _ = update(&mut st, Message::Done("plex".into(), "install", card("plex", false), Ok(card("plex", true))), ctx);
            assert!(st.acting.is_empty());
            assert!(st.open.contains("plex"), "a new plugin's card opens");
            // a failure is said on the card, in full
            let _ = update(&mut st, Message::Done("plex".into(), "update", card("plex", true), Err(ApiError { status: 400, message: "Plex names no place to update from".into(), data: Value::Null })), ctx);
            assert_eq!(st.outs["plex"], (false, "Plex names no place to update from".to_string()));
            // a plugin busy on the server makes the page look again
            let mut busy = card("plex", true);
            busy["busy"] = json!("updating");
            set_card(&mut st, busy);
            assert!(anything_busy(&st));
            // Remove asks first, with the catalogue's words
            let _ = update(&mut st, Message::Act("plex".into(), "remove"), ctx);
            assert!(matches!(st.dialog, Some(Dialog::Remove { ref title, ref text, .. }) if title == "Remove plex" && text.contains("your settings for it are kept")));
            // Add a plugin: the address is looked at before the warning
            let _ = update(&mut st, Message::Url("http://example.org/p.zip".into()), ctx);
            let _ = update(&mut st, Message::InstallUrl, ctx);
            assert_eq!(st.add_out, Some((Some(false), "Plugins install only from https addresses".to_string())));
            let _ = update(&mut st, Message::Url("owner/repo@v1".into()), ctx);
            let _ = update(&mut st, Message::InstallUrl, ctx);
            assert!(matches!(st.dialog, Some(Dialog::Trust(Add::Url(ref u))) if u == "owner/repo@v1"));
        });
    }
}
