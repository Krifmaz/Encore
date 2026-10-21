//! The Settings → Server pages drawn from the schema are the pages the static table of 0.28 drew.
//!
//! fixtures/settings-schema.json is the answer of GET /api/admin/settings/schema saved from the server. The
//! values are made from its defaults, plus a case with what the form treats differently: values environment
//! variables set (locked), saved secrets, a secret that is not set, values saved in the UI, other torrent and
//! Usenet clients (the `when` rules), escaping, and keys this server does not have (fields and whole cards
//! left out). Every page is drawn for both cases, with and without the eye and copy buttons.
//!
//! fixtures/settings-cards.html is what the static table (web-rs/src/pages/settings.rs `SECTIONS`/`GROUPS`
//! until 0.29) drew for them, icons and the cards' own extra blocks shortened to a marker. It was written while
//! both renderers were here, after a test had compared them for every page and case and found the same HTML
//! byte for byte; the schema renderer has to keep drawing it. When a change to the form is meant, run
//! `ENCORE_WRITE_SETTINGS_CARDS=1 cargo test settings::` to write the file again and read its diff. R2 wrote it
//! again when Apps & sources became Library, Downloads, Integrations and Server processing: the same cards, on
//! those four pages, without the part headings, and the Tidal Client ID's help naming Downloads. 0.32 wrote it
//! again when the source cards of Downloads became plugin cards: Downloads keeps Download pipeline and Release
//! types (plugins_tests.rs holds the moved cards' fields to what this file had for them).
//! (The schema fixture is recorded by the server's config tests: `ENCORE_WRITE_SETTINGS_SCHEMA=1 cargo test -p
//! encore-config schema_fixture` in server/.)

use super::apps::{cadenza_cache_block, engine_settings_block, soulseek_status_block, streamrip_config_block, tidal_block};
use super::fields::{Schema, When};
use super::groups::{cards_html, page_cards, visible_cards};
use crate::json::J;
use serde_json::{json, Map, Value};

const SCHEMA_JSON: &str = include_str!("fixtures/settings-schema.json");
const CARDS: &str = "src/settings/fixtures/settings-cards.html";

pub(super) fn schema_json() -> Value { serde_json::from_str(SCHEMA_JSON).expect("the schema fixture is JSON") }
pub(super) fn schema() -> Schema { Schema::from_json(&schema_json()) }

/// Every field of the schema answer, as JSON.
fn all_fields(v: &Value) -> Vec<Value> {
    v.a("sections").iter().flat_map(|s| s.a("groups")).flat_map(|g| g.a("fields")).collect()
}

/// The field `key` of a schema answer, to change it.
fn field_mut<'a>(v: &'a mut Value, key: &str) -> &'a mut Value {
    v["sections"].as_array_mut().unwrap().iter_mut()
        .flat_map(|s| s["groups"].as_array_mut().unwrap().iter_mut())
        .flat_map(|g| g["fields"].as_array_mut().unwrap().iter_mut())
        .find(|f| f["key"] == key).expect("a field of the schema")
}

/// GET /admin/settings for the cases: (name, values with `_meta`).
pub(super) fn cases() -> Vec<(&'static str, Value)> {
    let mut values = Map::new();
    let mut meta = Map::new();
    for f in all_fields(&schema_json()) {
        values.insert(f.s("key"), f.g("default").clone());
        meta.insert(f.s("key"), json!({ "env": f.g("env"), "source": "default" }));
    }
    let defaults = { let mut v = values.clone(); v.insert("_meta".into(), Value::Object(meta.clone())); Value::Object(v) };

    let mut set = |k: &str, v: Value, source: &str| {
        values.insert(k.to_string(), v);
        if let Some(m) = meta.get_mut(k) { m["source"] = json!(source); }
    };
    set("library_path", json!("/music"), "env");
    set("github_token", json!("•••• (saved)"), "env");
    set("discord_webhook", json!("•••• (saved)"), "ui");
    set("vpn_wireguard_config", json!("•••• (saved)"), "ui");
    set("ytdlp_cookies", json!("•••• (saved)"), "ui");
    set("lastfm_api_key", json!("•••• (saved)"), "ui");
    set("app_name", json!("My <Music> & \"more\""), "ui");
    set("prowlarr_url", json!("http://prowlarr:9696/?a=\"b\"&c<d>"), "ui");
    set("torrent_client", json!("deluge"), "ui");
    set("nzb_client", json!("blackhole"), "ui");
    set("release_types", json!("Album,EP,single"), "ui");
    set("release_skip", json!("Live,Remix,Spoken word"), "ui");
    set("streamrip_quality", json!(4), "ui");
    set("cadenza_max_rate", json!(384000), "ui");
    set("auto_approve", json!(true), "ui");
    set("ytdlp_enabled", json!(false), "ui");
    set("musicbrainz_rate", json!(0.5), "ui");
    set("autotag_threshold", json!(0.85), "ui");
    // a secret with no environment variable of its own
    if let Some(m) = meta.get_mut("acoustid_api_key") { m["env"] = Value::Null; }
    // what this server does not have: two fields, a whole card (Roon) and a whole card on a page of two (Email)
    for k in ["sync_interval_minutes", "staging_path", "roon_enabled", "roon_host", "smtp_host", "smtp_port", "smtp_user", "smtp_password", "smtp_from", "smtp_security"] {
        values.remove(k);
    }
    let changed = { let mut v = values; v.insert("_meta".into(), Value::Object(meta)); Value::Object(v) };
    vec![("defaults", defaults), ("changed", changed)]
}

/// Icons as `<svg/>`, and the cards' own blocks (drawn by apps.rs, not from the schema) as a marker, so the file
/// records only what the schema decides.
fn normalized(html: &str) -> String {
    let mut s = html.to_string();
    for (name, block) in [
        ("YouTube sign-in", crate::pages::youtube::youtube_block()), ("Tidal account", tidal_block()),
        ("streamrip config file", streamrip_config_block()), ("Pre-render cache", cadenza_cache_block()),
        ("Engine settings", engine_settings_block()), ("Soulseek status", soulseek_status_block()),
    ] {
        s = s.replace(&block, &format!("<!-- {name} block -->"));
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find("<svg") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</svg>") {
            Some(j) => { out.push_str("<svg/>"); rest = &rest[i + j + "</svg>".len()..]; }
            None => { out.push_str(&rest[i..]); rest = ""; }
        }
    }
    out.push_str(rest);
    out
}

/// Every Server page for every case, with and without the eye and copy buttons, drawn by `page`.
fn all_pages(page: impl Fn(&str, &Value, bool, Option<&str>) -> String) -> String {
    let schema = schema();
    let mut out = String::new();
    for (case, values) in cases() {
        for reveal in [false, true] {
            for sec in &schema.sections {
                out.push_str(&format!("<!-- page {}, {case} values, reveal {reveal} -->\n", sec.id));
                out.push_str(&page(&sec.id, &values, reveal, None));
                out.push('\n');
            }
        }
    }
    // a link that names a card opens that one instead of the first
    let values = &cases()[0].1;
    out.push_str("<!-- page downloads, defaults, the Release types card asked for -->\n");
    out.push_str(&page("downloads", values, false, Some("releases")));
    out.push('\n');
    out
}

/// The schema renderer for one page.
fn from_schema(schema: &Schema) -> impl Fn(&str, &Value, bool, Option<&str>) -> String + '_ {
    move |id, values, reveal, focus| {
        let page = schema.section(id).expect("a page of the schema");
        cards_html(&page_cards(&page.groups, values), values, reveal, focus)
    }
}

/// The nav entries of Settings → Server: the static table's GROUPS of 0.28, with Apps & sources split into
/// Library, Downloads, Integrations and Server processing (R2, the owner on Oct 19).
const NAV: &[(&str, &str)] = &[
    ("general", "General"), ("library", "Library"), ("downloads", "Downloads"), ("integrations", "Integrations"),
    ("processing", "Server processing"), ("accounts", "Email & notifications"), ("autodownload", "Auto-download"),
    ("contribute", "Contribute"), ("blocked", "Blocked"), ("tasks", "Scheduled tasks"),
];

/// `#/settings?s=` names and the page they open: every name of 0.28 (pages, their aliases and the cards on them)
/// still opens the page that holds what it named, `apps` opens Downloads; anything else stays as it is (the hub
/// then shows Account, or General for about/api/keys).
const LINKS: &[(&str, &str)] = &[
    ("general", "general"), ("grouplisten", "general"), ("apps", "downloads"), ("downloads", "downloads"), ("tagging", "library"), ("services", "integrations"),
    ("library", "library"), ("verify", "library"),
    ("pipeline", "downloads"), ("releases", "downloads"), ("streamrip", "plugins"), ("torznab", "plugins"), ("slskd", "plugins"),
    ("ytdlp", "plugins"), ("youtube", "plugins"),
    ("integrations", "integrations"), ("spotify", "integrations"), ("media", "integrations"), ("roon", "integrations"),
    ("processing", "processing"), ("cadenza", "processing"),
    ("accounts", "accounts"), ("email", "accounts"), ("notify", "accounts"), ("autodownload", "autodownload"),
    ("contribute", "contribute"), ("blocked", "blocked"), ("tasks", "tasks"), ("sync", "tasks"),
    ("about", "about"), ("api", "api"), ("keys", "keys"), ("account", "account"), ("users", "users"), ("nothing", "nothing"),
];

#[test]
fn the_nav_is_the_split_one_and_old_links_still_open() {
    let schema = schema();
    let ours: Vec<(&str, &str)> = schema.sections.iter().map(|s| (s.id.as_str(), s.title.as_str())).collect();
    assert_eq!(ours, NAV);
    for (id, page) in LINKS { assert_eq!(super::page_for(&schema, id), *page, "?s={id}"); }
    // every card of the schema is one of them
    for g in schema.sections.iter().flat_map(|s| &s.groups) { assert!(LINKS.iter().any(|(id, _)| *id == g.id), "card {}", g.id); }
    // How requests are fulfilled goes on the page with the Download pipeline card (and Plugins after it in the nav)
    let with_tools: Vec<&str> = schema.sections.iter().map(|s| s.id.as_str()).filter(|id| schema.holds(id, "pipeline")).collect();
    assert_eq!(with_tools, ["downloads"]);
    // a page id wins over a card of the same name on another page (Library's own card is called library too)
    let mut moved = schema_json();
    let lib = moved["sections"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "library").unwrap()["groups"].as_array_mut().unwrap().remove(0);
    moved["sections"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "general").unwrap()["groups"].as_array_mut().unwrap().push(lib);
    assert_eq!(Schema::from_json(&moved).page_of("library"), "library");
    // a link that names a page by an alias (`apps`, `tagging`, `services`) opens that page's first card
    let values = &cases()[0].1;
    for (alias, page) in [("apps", "downloads"), ("tagging", "library"), ("services", "integrations")] {
        let html = from_schema(&schema)(page, values, false, Some(alias));
        assert_eq!(html, from_schema(&schema)(page, values, false, None), "?s={alias}");
        let first = visible_cards(&schema.section(page).unwrap().groups, values)[0].0.id.clone();
        assert!(html.contains(&format!(r#"data-sec="{first}" open>"#)), "?s={alias} opens {first}");
    }
}

#[test]
fn the_cards_are_the_ones_the_static_table_drew() {
    let schema = schema();
    let ours = normalized(&all_pages(from_schema(&schema)));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CARDS);
    if std::env::var_os("ENCORE_WRITE_SETTINGS_CARDS").is_some() { std::fs::write(&path, &ours).expect("write the cards file"); }
    let want = std::fs::read_to_string(&path).expect("the cards file");
    assert_same(&want, &ours, CARDS, "the schema renderer");
}

/// The first line where two drawings differ, with the lines around it.
fn assert_same(want: &str, got: &str, want_name: &str, got_name: &str) {
    if want == got { return; }
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    let at = w.iter().zip(&g).position(|(a, b)| a != b).unwrap_or(w.len().min(g.len()));
    let around = |l: &[&str]| l[at.saturating_sub(3)..(at + 3).min(l.len())].join("\n");
    panic!("{got_name} draws something else than {want_name} at line {}:\n--- {want_name}\n{}\n--- {got_name}\n{}", at + 1, around(&w), around(&g));
}

#[test]
fn when_rules_read_like_the_server_writes_them() {
    assert_eq!(When::parse("torrent_client=qbittorrent"), Some(When { key: "torrent_client".into(), value: "qbittorrent".into(), equal: true }));
    assert_eq!(When::parse("nzb_client!=blackhole"), Some(When { key: "nzb_client".into(), value: "blackhole".into(), equal: false }));
    assert_eq!(When::parse("nothing"), None);
    assert_eq!(When::parse("=x"), None);
    assert_eq!(When::parse("k="), None);
    let w = When::parse("nzb_client!=blackhole").unwrap();
    assert!(w.shows("sabnzbd") && !w.shows("blackhole"));
}

#[test]
fn a_not_equal_rule_is_drawn_and_hides_its_field() {
    let mut v = schema_json();
    field_mut(&mut v, "qbit_url")["when"] = json!("torrent_client!=blackhole");
    let schema = Schema::from_json(&v);
    let page = schema.section("downloads").unwrap();
    let draw = |client: &str| {
        let mut values = cases()[0].1.clone();
        values["torrent_client"] = json!(client);
        cards_html(&visible_cards(&page.groups, &values), &values, false, None)
    };
    let shown = r#"<label data-when="torrent_client" data-when-not="blackhole" class="field "><span>qBittorrent URL</span>"#;
    let hidden = r#"<label data-when="torrent_client" data-when-not="blackhole" hidden class="field "><span>qBittorrent URL</span>"#;
    assert!(draw("deluge").contains(shown));
    assert!(draw("blackhole").contains(hidden));
}

#[test]
fn every_field_type_of_the_schema_is_drawn() {
    // path and url are text inputs; a type this web app does not know yet is one too
    let mut v = schema_json();
    field_mut(&mut v, "app_name")["type"] = json!("colour");
    let schema = Schema::from_json(&v);
    let values = &cases()[0].1;
    let html = from_schema(&schema)("general", values, false, None);
    assert!(html.contains(r#"<input class="input" type="text" data-k="app_name" "#), "{html}");
    assert!(html.contains(r#"<input class="input" type="text" data-k="base_url" "#), "url as text");
    let library = from_schema(&schema)("library", values, false, None);
    assert!(library.contains(r#"<input class="input" type="text" data-k="staging_path" "#), "path as text");
}
