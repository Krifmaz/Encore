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
//! `ENCORE_WRITE_SETTINGS_CARDS=1 cargo test settings::` to write the file again and read its diff.

use super::apps::{cadenza_cache_block, engine_settings_block, soulseek_status_block, streamrip_config_block, tidal_block};
use super::fields::{Schema, When};
use super::groups::{cards_html, visible_cards};
use crate::json::J;
use serde_json::{json, Map, Value};

const SCHEMA_JSON: &str = include_str!("fixtures/settings-schema.json");
const CARDS: &str = "src/settings/fixtures/settings-cards.html";

fn schema_json() -> Value { serde_json::from_str(SCHEMA_JSON).expect("the schema fixture is JSON") }
fn schema() -> Schema { Schema::from_json(&schema_json()) }

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
fn cases() -> Vec<(&'static str, Value)> {
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
    out.push_str("<!-- page apps, defaults, the Soulseek card asked for -->\n");
    out.push_str(&page("apps", values, false, Some("slskd")));
    out.push('\n');
    out
}

/// The schema renderer for one page.
fn from_schema(schema: &Schema) -> impl Fn(&str, &Value, bool, Option<&str>) -> String + '_ {
    move |id, values, reveal, focus| {
        let page = schema.section(id).expect("a page of the schema");
        cards_html(&visible_cards(&page.groups, values), values, reveal, focus)
    }
}

/// The nav entries of Settings → Server in 0.28 (the static table's GROUPS).
const NAV_028: &[(&str, &str)] = &[
    ("general", "General"), ("apps", "Apps & sources"), ("accounts", "Email & notifications"), ("autodownload", "Auto-download"),
    ("contribute", "Contribute"), ("blocked", "Blocked"), ("tasks", "Scheduled tasks"),
];

/// `#/settings?s=` names and the page they opened in 0.28: pages, their aliases and the cards on them; anything
/// else stays as it is (the hub then shows Account, or General for about/api/keys).
const LINKS_028: &[(&str, &str)] = &[
    ("general", "general"), ("apps", "apps"), ("downloads", "apps"), ("tagging", "apps"), ("services", "apps"),
    ("pipeline", "apps"), ("releases", "apps"), ("streamrip", "apps"), ("torznab", "apps"), ("slskd", "apps"), ("ytdlp", "apps"),
    ("youtube", "apps"), ("library", "apps"), ("verify", "apps"), ("spotify", "apps"), ("media", "apps"), ("roon", "apps"),
    ("cadenza", "apps"), ("accounts", "accounts"), ("email", "accounts"), ("notify", "accounts"), ("autodownload", "autodownload"),
    ("contribute", "contribute"), ("blocked", "blocked"), ("tasks", "tasks"), ("sync", "tasks"),
    ("about", "about"), ("api", "api"), ("keys", "keys"), ("account", "account"), ("users", "users"), ("nothing", "nothing"),
];

#[test]
fn the_nav_and_old_links_are_the_ones_of_0_28() {
    let schema = schema();
    let ours: Vec<(&str, &str)> = schema.sections.iter().map(|s| (s.id.as_str(), s.title.as_str())).collect();
    assert_eq!(ours, NAV_028);
    for (id, page) in LINKS_028 { assert_eq!(schema.page_of(id), *page, "?s={id}"); }
    // every card of the schema is one of them
    for g in schema.sections.iter().flat_map(|s| &s.groups) { assert!(LINKS_028.iter().any(|(id, _)| *id == g.id), "card {}", g.id); }
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
    let page = schema.section("apps").unwrap();
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
    let apps = from_schema(&schema)("apps", values, false, None);
    assert!(apps.contains(r#"<input class="input" type="text" data-k="streamrip_bin" "#), "path as text");
}
