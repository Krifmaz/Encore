//! The catalog test: every manifest Encore ships reads and passes the catalog's rules, ids, source ids and
//! setting keys are unique, every setting key is one of Encore's (settings.toml) or new and documented in
//! docs/contracts/plugin-manifest.md, and nothing of the cards and tools the plugins replace is left behind.
use crate::catalog::{catalog, find, for_old_tool, for_source, text, FILES};
use crate::manifest::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const SETTINGS_TOML: &str = include_str!("../../config/settings.toml");
const MANIFEST_DOC: &str = include_str!("../../../../docs/contracts/plugin-manifest.md");

/// The settings table: key → its attributes.
fn encore_keys() -> BTreeMap<String, toml::Table> {
    let t: toml::Table = toml::from_str(SETTINGS_TOML).unwrap();
    t["keys"].as_table().unwrap().iter().map(|(k, v)| (k.clone(), v.as_table().unwrap().clone())).collect()
}

/// The pages (sections) of the settings table.
fn encore_sections() -> BTreeSet<String> {
    let t: toml::Table = toml::from_str(SETTINGS_TOML).unwrap();
    t["sections"].as_table().unwrap().keys().cloned().collect()
}

/// The keys the manifest doc lists under "New setting keys" (`- `key` ...` lines; "None" when there are none).
fn documented_new_keys() -> BTreeSet<String> {
    let start = MANIFEST_DOC.find("## New setting keys").expect("the doc has a \"New setting keys\" section");
    let section = &MANIFEST_DOC[start..];
    let section = &section[..section[3..].find("\n## ").map(|i| i + 3).unwrap_or(section.len())];
    section.lines().filter_map(|l| l.strip_prefix("- `")).filter_map(|l| l.split('`').next()).map(str::to_string).collect()
}

fn all() -> Vec<&'static Manifest> {
    catalog().iter().collect()
}

#[test]
fn every_catalog_manifest_reads() {
    for (id, body) in FILES {
        let m = parse_for(body, Origin::Catalog).unwrap_or_else(|e| panic!("catalog/{id}.toml: {e}"));
        assert_eq!(m.id(), id, "catalog/{id}.toml names the plugin it holds");
        assert!(m.plugin.builtin, "{id}: the 0.32 catalog is Encore's own code");
        assert!(!m.plugin.description.is_empty() && !m.plugin.homepage.is_empty() && !m.plugin.license.is_empty(), "{id}: description, homepage, license");
        assert!(m.plugin.commands.iter().any(|c| c == "status"), "{id}: every plugin card shows a status");
        assert_eq!(text(id), Some(body));
    }
    assert_eq!(catalog().len(), FILES.len(), "nothing left out");
    let ids: Vec<&str> = catalog().iter().map(|m| m.id()).collect();
    assert_eq!(
        ids,
        [
            "ytdlp", "streamrip", "slskd", "zotify", "gamdl", "torznab", "qbittorrent", "transmission", "deluge", "rtorrent", "torrent-blackhole", "sabnzbd", "nzbget",
            "usenet-blackhole", "cadenza", "translator", "jellyfin", "plex", "navidrome"
        ]
    );
    // every catalog file is listed (a new file in catalog/ that the list misses would never ship)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let listed: BTreeSet<String> = FILES.iter().map(|(id, _)| format!("{id}.toml")).collect();
    assert_eq!(on_disk, listed);
}

#[test]
fn ids_sources_and_setting_keys_are_unique() {
    let p = check_together(&all());
    assert!(p.is_empty(), "{p:?}");
    let keys: Vec<String> = catalog().iter().flat_map(|m| m.settings.iter().map(|s| m.store_key(s))).collect();
    assert_eq!(keys.len(), keys.iter().collect::<BTreeSet<_>>().len());
}

#[test]
fn every_setting_key_is_encores_or_new_and_documented() {
    let encore = encore_keys();
    let new_keys = documented_new_keys();
    for m in catalog() {
        for s in &m.settings {
            let at = format!("{} {}", m.id(), s.key);
            let words = s.label.is_some() || s.kind.is_some() || s.help.is_some() || !s.options.is_empty() || s.default.is_some() || s.min.is_some() || s.max.is_some() || s.unit.is_some() || s.step.is_some() || s.when.is_some();
            match encore.get(&s.key) {
                // a field of the settings table: its words, type, env lock and secret flag come from there
                Some(def) if def.contains_key("group") => assert!(!words, "{at}: the settings table has this field's words; the manifest names only the key"),
                // no field there (internal, or drawn by the old Download tools card): all of a field's words, or none
                Some(def) => {
                    if words {
                        assert!(s.label.is_some() && s.kind.is_some(), "{at}: a field drawn from the manifest has a label and a type");
                        if let (Some(d), Some(enc)) = (&s.default, def.get("default")) {
                            assert_eq!(&toml_json(enc), d, "{at}: the manifest's default is Encore's");
                        }
                    }
                }
                None => {
                    assert!(new_keys.contains(&s.key), "{at}: not an Encore setting, and not under \"New setting keys\" in docs/contracts/plugin-manifest.md");
                    assert!(s.label.is_some() && s.kind.is_some(), "{at}: a new key brings its label and type");
                }
            }
        }
        for k in m.named_keys() {
            assert!(encore.contains_key(&k) || new_keys.contains(&k), "{}: {k} (in a rule, a path grant or the auto switch) is not an Encore setting", m.id());
        }
    }
    for k in &new_keys {
        assert!(catalog().iter().any(|m| m.setting(k).is_some()), "the doc lists {k} as new, but no plugin has it");
    }
}

/// A settings.toml value as JSON.
fn toml_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::from(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::from(*b),
        other => Value::from(format!("{other:?}")),
    }
}

#[test]
fn the_cards_that_move_lose_no_field() {
    let encore = encore_keys();
    let owner = |k: &str| catalog().iter().filter(|m| m.setting(k).is_some()).map(|m| m.id()).collect::<Vec<_>>();
    for (key, def) in &encore {
        let group = def.get("group").and_then(|g| g.as_str()).unwrap_or("");
        if ["streamrip", "torznab", "slskd", "ytdlp", "youtube", "media", "cadenza"].contains(&group) {
            assert_eq!(owner(key).len(), 1, "{key} (card {group}) belongs to exactly one plugin");
        }
        if key.starts_with("tools_auto_") {
            let takers: Vec<&str> =
                catalog().iter().filter(|m| m.migration.as_ref().and_then(|g| g.auto_key.as_deref()) == Some(key.as_str())).map(|m| m.id()).collect();
            assert_eq!(takers.len(), 1, "{key}: one plugin takes the old auto-update switch over");
        }
    }
    for key in ["translator_model", "translate_background", "streamrip_sources", "slskd_managed_api_key", "plex_client_id", "cadenza_engine_settings"] {
        assert_eq!(owner(key).len(), 1, "{key} belongs to one plugin");
    }
    // the moved cards are drawn on the plugin's card; the ones that stay say where
    for id in ["ytdlp", "streamrip", "slskd", "torznab", "translator"] {
        assert_eq!(find(id).unwrap().plugin.settings_on, None, "{id}: its fields move into its card");
    }
    let sections = encore_sections();
    for m in catalog() {
        if let Some(s) = &m.plugin.settings_on {
            assert!(sections.contains(s), "{}: settings_on {s} is not a page of the settings table", m.id());
        }
    }
    assert_eq!(find("cadenza").unwrap().plugin.settings_on.as_deref(), Some("processing"));
    for id in ["jellyfin", "plex", "navidrome"] {
        assert_eq!(find(id).unwrap().plugin.settings_on.as_deref(), Some("integrations"));
    }
}

#[test]
fn the_old_tools_and_sources_are_all_taken_over() {
    for tool in OLD_TOOLS {
        let m = for_old_tool(tool).unwrap_or_else(|| panic!("no plugin takes over the {tool} tool"));
        assert_eq!(m.id(), tool, "/api/system/tools/{tool} keeps its id");
        assert_eq!(m.migration.as_ref().unwrap().auto_key.as_deref(), Some(format!("tools_auto_{tool}").as_str()));
    }
    // every id source_order and the picker use today
    for (source, plugin) in [("qobuz", "streamrip"), ("tidal", "streamrip"), ("deezer", "streamrip"), ("slskd", "slskd"), ("torznab", "torznab"), ("ytdlp", "ytdlp"), ("ytmusic", "ytdlp"), ("zotify", "zotify"), ("gamdl", "gamdl")] {
        assert_eq!(for_source(source).map(|m| m.id()), Some(plugin), "{source}");
    }
    let order = encore_keys()["source_order"]["default"].as_str().unwrap().to_string();
    for id in order.split(',') {
        assert!(for_source(id.trim()).is_some(), "the default source order names {id}");
    }
    // the folders the old tools used, adopted in place
    let adopted: BTreeSet<&str> = catalog().iter().flat_map(|m| m.migration.iter().flat_map(|g| g.adopt.iter().map(String::as_str))).collect();
    assert_eq!(adopted, BTreeSet::from(["tools/venv", "tools/slskd", "slskd", "tools/cadenza", "tools/translator", "streamrip"]));
}

#[test]
fn every_program_has_both_machines_a_sum_and_a_version_check() {
    for m in catalog().iter().filter(|m| m.installs_program()) {
        let id = m.id();
        let u = m.update.as_ref().unwrap_or_else(|| panic!("{id}: [update]"));
        assert_ne!(u.source(), "none", "{id}: where new versions come from");
        // the programs of 0.32 took over a tool of the old Download tools card; zotify and gamdl (0.34) are new
        if !crate::catalog::THROUGH_REGISTRY.contains(&id) {
            assert!(m.migration.as_ref().and_then(|g| g.from_tool.as_ref()).is_some(), "{id}: a program the old Download tools card had");
        }
        let argv = m.verify_argv();
        assert_eq!(argv[0], "{bin}", "{id}: the version command runs the program");
        if !m.binaries.is_empty() {
            for arch in ARCHES {
                let b = m.binary_for(arch).unwrap_or_else(|| panic!("{id}: no program for {arch}"));
                assert!(b.sha256.is_some() || b.sums.is_some() || u.sums.is_some(), "{id} {arch}: a sum to check");
                for name in b.names() {
                    assert!(b.unpack_for(name).is_some(), "{id} {arch}: how {name} is unpacked");
                    let filled = fill(name, "1.2.3", &u.tag_for("1.2.3"));
                    assert!(!filled.contains('{'), "{id} {arch}: {name} has a placeholder Encore does not fill");
                }
            }
        }
        if let Some(py) = &m.python {
            assert_eq!(py.python, "3.14", "{id}: the image's Python");
            for pkg in &py.packages {
                assert!(pkg.contains("{version}") || pkg.contains("{tag}"), "{id}: {pkg} installs the version that was found");
            }
        }
    }
    // the version-and-switch check the image ran on streamrip, now the plugin's
    let rip = find("streamrip").unwrap();
    let u = rip.update.as_ref().unwrap();
    assert_eq!((u.pin.as_deref(), u.pin_reports.as_deref()), (Some("v2.4.10"), Some("2.4.9")));
    let check = &rip.install.as_ref().unwrap().checks[0];
    assert_eq!((check.file.as_deref(), check.contains.as_deref()), (Some("lib/python3*/site-packages/streamrip/config.py"), Some("no_update_check")));
    assert_eq!(rip.python.as_ref().unwrap().bin.0, ["rip", "streamrip"]);
    // Cadenza's render protocol
    let cz = find("cadenza").unwrap();
    assert_eq!(cz.verify_argv(), ["{bin}", "render", "--describe"]);
    assert_eq!(cz.install.as_ref().unwrap().checks[0].equals, Some(Value::from(1)));
    // the translator's two models, one per choice
    let tr = find("translator").unwrap();
    let whens: BTreeSet<&str> = tr.downloads.iter().filter_map(|d| d.when.as_deref()).collect();
    assert_eq!(whens, BTreeSet::from(["translator_model=best", "translator_model=small"]));
    let options: BTreeSet<&str> = tr.setting("translator_model").unwrap().options.iter().map(|(v, _)| v.as_str()).collect();
    assert_eq!(options, BTreeSet::from(["best", "small"]));
}

#[test]
fn clients_name_their_source_and_a_choice_the_settings_offer() {
    let encore = encore_keys();
    let mut by_protocol: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for m in catalog().iter().filter(|m| m.plugin.kind == "client") {
        let c = m.client.as_ref().unwrap();
        let source = find(&c.source).unwrap_or_else(|| panic!("{}: its source {} is not in the catalog", m.id(), c.source));
        assert_eq!(source.plugin.kind, "source");
        let Ok(Condition::Is(key, value)) = Condition::parse(&c.chosen_by) else { panic!("{}: chosen_by", m.id()) };
        assert!(source.setting(&key).is_some(), "{}: {key} is a setting of {}", m.id(), c.source);
        let options = encore[&key]["options"].as_array().unwrap();
        assert!(options.iter().any(|o| o[0].as_str() == Some(value.as_str())), "{}: {value} is a choice of {key}", m.id());
        by_protocol.entry(key).or_default().insert(value);
    }
    // every choice of the two selects has its plugin
    for (key, chosen) in by_protocol {
        let offered: BTreeSet<String> = encore[&key]["options"].as_array().unwrap().iter().map(|o| o[0].as_str().unwrap().to_string()).collect();
        assert_eq!(chosen, offered, "{key}");
    }
}

/// The settings of a config as a fresh 0.30 install leaves it (the defaults), with some set by the owner.
struct Config {
    defaults: BTreeMap<String, Value>,
    set: BTreeMap<&'static str, Value>,
    files: BTreeSet<&'static str>,
}

impl Config {
    fn new() -> Config {
        let defaults = encore_keys().into_iter().map(|(k, t)| (k, toml_json(&t["default"]))).collect();
        Config { defaults, set: BTreeMap::new(), files: BTreeSet::new() }
    }
    fn with(mut self, key: &'static str, v: Value) -> Config {
        self.set.insert(key, v);
        self
    }
    fn file(mut self, rel: &'static str) -> Config {
        self.files.insert(rel);
        self
    }
    fn in_use(&self) -> Vec<&'static str> {
        catalog().iter().filter(|m| m.in_use(self)).map(|m| m.id()).collect()
    }
}

impl Facts for Config {
    fn value(&self, key: &str) -> Option<Value> {
        self.set.get(key).cloned().or_else(|| self.defaults.get(key).cloned())
    }
    fn saved(&self, key: &str) -> bool {
        self.set.contains_key(key)
    }
    fn exists(&self, rel: &str) -> bool {
        self.files.contains(rel)
    }
}

#[test]
fn an_upgrade_takes_over_what_was_in_use() {
    use serde_json::json;
    // an untouched 0.30 config: yt-dlp was on (it is by default), nothing else was set up
    assert_eq!(Config::new().in_use(), ["ytdlp"]);
    // yt-dlp with YouTube Music off too: every pasted link still went through the image's yt-dlp
    assert_eq!(Config::new().with("ytdlp_enabled", json!(false)).in_use(), ["ytdlp"]);
    let set_up = Config::new()
        .with("streamrip_enabled", json!(true))
        .with("streamrip_deezer_arl", json!("arl"))
        .with("slskd_enabled", json!(true))
        .with("soulseek_username", json!("me"))
        .with("soulseek_password", json!("pw"))
        .with("torznab_enabled", json!(true))
        .with("prowlarr_url", json!("http://prowlarr:9696"))
        .with("prowlarr_api_key", json!("k"))
        .with("qbit_url", json!("http://qbittorrent:8080"))
        .with("nzb_client", json!("blackhole"))
        .with("jellyfin_url", json!("http://jellyfin:8096"));
    assert_eq!(set_up.in_use(), ["ytdlp", "streamrip", "slskd", "torznab", "qbittorrent", "usenet-blackhole", "jellyfin"]);
    // a tool whose folder is there, or whose auto-update the owner switched on
    let tools = Config::new()
        .with("ytdlp_enabled", json!(false))
        .file("tools/venv/bin/rip")
        .file("tools/cadenza/current")
        .with("tools_auto_slskd", json!(true))
        .file("tools/translator/llama/current")
        .file("tools/translator/models");
    assert_eq!(tools.in_use(), ["ytdlp", "streamrip", "slskd", "cadenza", "translator"]);
    // the translator only with its model: a model is gigabytes
    let no_model = Config::new().with("ytdlp_enabled", json!(false)).with("tools_auto_translator", json!(true)).file("tools/translator/llama/current");
    assert_eq!(no_model.in_use(), ["ytdlp"]);
    // a client set up while torrents were off is not in use
    let off = Config::new().with("ytdlp_enabled", json!(false)).with("qbit_url", json!("http://qbittorrent:8080"));
    assert_eq!(off.in_use(), ["ytdlp"]);
    // the client Encore really picked: any torrent_client value that names no other client is qBittorrent,
    // "rutorrent" is rTorrent, any nzb_client value that names no other is SABnzbd
    let torrents = |client: &'static str| {
        Config::new().with("torznab_enabled", json!(true)).with("torrent_client", json!(client)).with("nzb_client", json!(" "))
            .with("qbit_url", json!("http://qbittorrent:8080")).with("rtorrent_url", json!("http://rtorrent/RPC2")).with("sab_url", json!("http://sab:8080"))
            .in_use()
    };
    assert_eq!(torrents("qbit"), ["ytdlp", "qbittorrent", "sabnzbd"]);
    assert_eq!(torrents(""), ["ytdlp", "qbittorrent", "sabnzbd"]);
    assert_eq!(torrents("ruTorrent"), ["ytdlp", "rtorrent", "sabnzbd"]);
    assert_eq!(torrents("rtorrent"), ["ytdlp", "rtorrent", "sabnzbd"]);
    assert_eq!(torrents("deluge"), ["ytdlp", "sabnzbd"], "deluge's address is not set");
}

#[test]
fn the_docs_examples_are_the_real_thing() {
    let blocks: Vec<&str> = MANIFEST_DOC.split("```toml\n").skip(1).filter_map(|b| b.split("\n```").next()).collect();
    let ytdlp = blocks.iter().find(|b| b.contains("id = \"ytdlp\"")).expect("the doc shows the yt-dlp manifest");
    assert_eq!(format!("{ytdlp}\n"), text("ytdlp").unwrap(), "the doc's yt-dlp example is catalog/ytdlp.toml as it is");
    let tone = blocks.iter().find(|b| b.contains("id = \"example-tone\"")).expect("the doc shows the example plugin's manifest");
    let m = parse_for(tone, Origin::Upload).unwrap_or_else(|e| panic!("the example plugin: {e}"));
    assert!(m.is_process() && m.plugin.kind == "source" && !m.permissions.network.allowed());
    let mut all_ids: Vec<&Manifest> = all();
    all_ids.push(&m);
    assert!(check_together(&all_ids).is_empty(), "the example fits beside the catalog");
    // the example's folder holds that manifest and the SDK of plugins/sdk, byte for byte
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let read = |rel: &str| std::fs::read_to_string(repo.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    assert_eq!(read("docs/plugins/example-tone/plugin.toml"), format!("{tone}\n"), "docs/plugins/example-tone/plugin.toml is the doc's example");
    assert_eq!(read("docs/plugins/example-tone/encore_plugin.py"), read("plugins/sdk/encore_plugin.py"), "the example carries the SDK as it is");
}

#[test]
fn the_sources_through_the_registry_are_catalog_sources_with_a_venv() {
    for id in crate::catalog::THROUGH_REGISTRY {
        let m = find(id).unwrap_or_else(|| panic!("{id} is in the catalog"));
        assert!(m.plugin.builtin && m.plugin.kind == "source", "{id}: a builtin source");
        assert!(m.python.is_some() && m.update.as_ref().and_then(|u| u.pin.as_ref()).is_some(), "{id}: a venv, Install pinned to a tested version");
        assert!(m.migration.is_none(), "{id}: new in 0.34, nothing to take over");
        assert_eq!(for_source(id).map(|m| m.id()), Some(id));
    }
    // zotify's librespot is a git URL in its own requirements: the archive of a fixed commit replaces it
    let z = find("zotify").unwrap().python.as_ref().unwrap();
    assert_eq!(z.overrides.len(), 1);
    assert!(z.overrides[0].starts_with("librespot @ https://github.com/Googolplexed0/librespot-python/archive/") && z.overrides[0].ends_with(".zip"));
    assert!(z.packages[0].contains("{tag}"));
}
