//! The settings table (settings.toml): its keys, defaults and rules, the code that reads settings, and the
//! schema answer. (The web app draws the Server pages from the schema since 0.29; web-rs keeps the recorded
//! HTML of the old static table and compares its renderer with it.)
use crate::settings::{def, default_of, Field, Keys, SCHEMA, TABLE};
use crate::{when, Config};
use encore_db::Db;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn fields() -> impl Iterator<Item = &'static Field> {
    SCHEMA.iter().flat_map(|s| s.groups.iter()).flat_map(|g| g.fields.iter())
}

fn options_of(key: &str) -> Vec<&'static str> {
    fields().find(|f| f.key == key).map(|f| f.options.iter().map(|o| o.0).collect()).unwrap_or_default()
}

#[test]
fn every_key_is_listed_once_and_every_field_is_a_key() {
    let mut seen = HashSet::new();
    for d in TABLE {
        assert!(seen.insert(d.key), "{} twice", d.key);
    }
    let mut on_cards = HashSet::new();
    for f in fields() {
        let d = def(f.key).unwrap_or_else(|| panic!("{} is on a card but not in TABLE", f.key));
        assert!(on_cards.insert(f.key), "{} is on two cards", f.key);
        assert_eq!(d.kind, f.kind, "{}", f.key);
        assert_eq!(d.label, Some(f.label), "{}", f.key);
    }
    // a key with a group is on that card, and only those are
    for d in TABLE {
        assert_eq!(d.group.is_some(), on_cards.contains(d.key), "{}", d.key);
    }
    assert_eq!(Keys::APP_NAME, "app_name");
    assert_eq!(Keys::TASK_BACKUP_MINUTES, "task_backup_minutes");
}

/// The TABLE order is the order the loader always had (the diagnostics page lists changed settings in it):
/// A to Z, then the settings added later in the order they were added.
#[test]
fn table_order_is_a_to_z_then_the_later_keys() {
    let keys: Vec<&str> = TABLE.iter().map(|d| d.key).collect();
    let first_late = keys.iter().position(|k| *k == "email_login").unwrap();
    assert!(keys[..first_late].windows(2).all(|w| w[0] < w[1]), "the first run is A to Z");
    assert_eq!(&keys[first_late..first_late + 3], &["email_login", "app_session_days", "lastfm_shared_secret"]);
    assert_eq!(keys.last(), Some(&"backup_keep_upgrade"));
}

#[test]
fn defaults_fit_their_fields() {
    for d in TABLE {
        let v = d.default.to_json();
        match d.kind {
            "bool" => assert!(v.is_boolean(), "{}", d.key),
            "number" => assert!(v.is_number(), "{}", d.key),
            "select" => {
                let text = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                assert!(options_of(d.key).contains(&text.as_str()), "{}: default {text} is not an option", d.key);
            }
            "checks" => {
                let opts = options_of(d.key);
                for part in v.as_str().unwrap().split(',') {
                    assert!(opts.contains(&part), "{}: {part} is not an option", d.key);
                }
            }
            _ => assert!(v.is_string(), "{}", d.key),
        }
        if d.kind == "password" {
            assert!(d.secret, "{}", d.key);
        }
        if d.secret && d.group.is_some() {
            assert!(matches!(d.kind, "password" | "textarea"), "{}: a secret is never shown in a plain field", d.key);
        }
    }
    for f in fields() {
        if let (Some(min), Some(max)) = (f.min, f.max) {
            assert!(min < max, "{}", f.key);
        }
        if let Some(v) = def(f.key).unwrap().default.to_json().as_f64() {
            assert!(f.min.is_none_or(|m| v >= m) && f.max.is_none_or(|m| v <= m), "{}: default {v} out of range", f.key);
        }
    }
}

#[test]
fn release_type_settings_match_release_types() {
    use crate::release_types::{default_skip, DEFAULT_KINDS, EXTRAS, KINDS};
    assert_eq!(default_of("release_types"), Some(json!(DEFAULT_KINDS)));
    assert_eq!(default_of("release_skip"), Some(json!(default_skip())));
    let pairs = |key: &str| fields().find(|f| f.key == key).unwrap().options.to_vec();
    assert_eq!(pairs("release_types"), KINDS.to_vec());
    assert_eq!(pairs("release_skip"), EXTRAS.to_vec());
}

#[test]
fn when_rules_parse_and_name_a_select_option() {
    assert_eq!(when::parse("torrent_client=qbittorrent"), Ok(when::When { key: "torrent_client", value: "qbittorrent", equal: true }));
    assert_eq!(when::parse("nzb_client!=blackhole"), Ok(when::When { key: "nzb_client", value: "blackhole", equal: false }));
    for bad in ["", "torrent_client", "=qbittorrent", "torrent_client=", "Torrent=x", "a b=c", "a=b=c", "a= b", "a!="] {
        assert!(when::parse(bad).is_err(), "{bad:?} should not parse");
    }
    let mut rules = 0;
    for f in fields() {
        let Some(rule) = f.when else { continue };
        let w = when::parse(rule).unwrap_or_else(|e| panic!("{}: {e}", f.key));
        assert_eq!(def(w.key).map(|d| d.kind), Some("select"), "{}: {rule}", f.key);
        assert!(options_of(w.key).contains(&w.value), "{}: {rule}", f.key);
        rules += 1;
    }
    assert!(rules >= 30, "the download clients' fields have rules ({rules})");
}

#[test]
fn typed_accessors_read_the_values_in_effect() {
    let d = encore_core::tmp::dir("settings-accessors");
    let cfg = Config::load(Db::open(&d).unwrap()).unwrap();
    assert_eq!(cfg.app_name(), "Encore");
    assert_eq!(cfg.session_days(), 30);
    assert!(!cfg.auto_approve());
    assert_eq!(cfg.musicbrainz_rate(), cfg.float("musicbrainz_rate"));
    assert_eq!(cfg.mb_oauth_expires_at(), 0.0);
    cfg.set_many(&json!({ "app_name": "Home music", "session_days": "12", "auto_approve": "yes" }));
    assert_eq!((cfg.app_name(), cfg.session_days(), cfg.auto_approve()), ("Home music".to_string(), 12, true));
    // the loader's table is the generated one, in its order, with the defaults coerced as before
    let keys: Vec<&str> = cfg.defs.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, TABLE.iter().map(|d| d.key).collect::<Vec<_>>());
    for ((k, d), t) in cfg.defs.iter().zip(TABLE) {
        assert_eq!((d.env.as_deref(), &d.default, d.secret), (t.env, &t.default.to_json(), t.secret), "{k}");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn schema_answer_has_every_field_with_its_attributes() {
    // NTFY_URL set in this config's own environment (the process's is not touched)
    let d = encore_core::tmp::dir("settings-schema");
    let cfg = Config::load_with(Db::open(&d).unwrap(), |name| (name == "NTFY_URL").then(|| "https://ntfy.example/topic".to_string())).unwrap();
    let v = cfg.schema();
    let ids: Vec<&str> = v["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["general", "library", "downloads", "integrations", "processing", "accounts", "autodownload", "contribute", "blocked", "tasks"]);
    let attrs = ["key", "label", "help", "type", "options", "when", "secret", "env", "env_locked", "default", "unit", "min", "max", "step", "full", "advanced"];
    let mut n = 0;
    let mut by_key: HashMap<String, Value> = HashMap::new();
    for s in v["sections"].as_array().unwrap() {
        assert!(s["title"].is_string() && s["aliases"].is_array());
        for g in s["groups"].as_array().unwrap() {
            for a in ["id", "title", "part", "desc", "test", "test_label", "fields"] {
                assert!(g.get(a).is_some(), "{} has no {a}", g["id"]);
            }
            for f in g["fields"].as_array().unwrap() {
                let keys: Vec<&String> = f.as_object().unwrap().keys().collect();
                assert_eq!(keys.len(), attrs.len(), "{f}");
                for a in attrs {
                    assert!(f.get(a).is_some(), "{} has no {a}", f["key"]);
                }
                by_key.insert(f["key"].as_str().unwrap().to_string(), f.clone());
                n += 1;
            }
        }
    }
    assert_eq!(n, fields().count());
    let ntfy = &by_key["ntfy_url"];
    assert_eq!((ntfy["env"].as_str(), ntfy["env_locked"].as_bool()), (Some("NTFY_URL"), Some(true)));
    assert_eq!(by_key["app_name"]["env_locked"], false);
    assert_eq!(by_key["smtp_password"]["secret"], true);
    assert_eq!(by_key["soulseek_port"]["max"], json!(65535));
    assert_eq!(by_key["musicbrainz_rate"]["step"], json!(0.1));
    assert_eq!(by_key["qbit_url"]["when"], "torrent_client=qbittorrent");
    assert_eq!(by_key["share_preview"]["options"][0], json!({ "value": "small", "label": "Small card (small cover)" }));
    assert_eq!(by_key["streamrip_quality"]["default"], json!(2));
    let _ = std::fs::remove_dir_all(d);
}

/// R2 (the owner, Oct 19): Apps & sources is four pages, and every link to the old page, its aliases and its
/// cards still opens the page that holds the card.
#[test]
fn apps_and_sources_is_four_pages_and_old_links_still_open() {
    let page_of = |name: &str| {
        SCHEMA.iter().find(|s| s.id == name || s.aliases.contains(&name) || s.groups.iter().any(|g| g.id == name)).map(|s| s.id)
    };
    let cards = |id: &str| SCHEMA.iter().find(|s| s.id == id).unwrap().groups.iter().map(|g| g.id).collect::<Vec<_>>();
    // 0.33: the sources of translated titles on the Library page
    assert_eq!(cards("library"), ["library", "verify", "translations", "surround", "trash"]);
    assert_eq!(cards("downloads"), ["pipeline", "releases", "streamrip", "torznab", "slskd", "ytdlp", "youtube"]);
    assert_eq!(cards("integrations"), ["spotify", "media", "roon"]);
    assert_eq!(cards("processing"), ["cadenza"]);
    let titles: Vec<&str> = SCHEMA.iter().map(|s| s.title).collect();
    assert_eq!(titles[..5], ["General", "Library", "Downloads", "Integrations", "Server processing"]);
    for (link, page) in [
        ("apps", "downloads"), ("downloads", "downloads"), ("tagging", "library"), ("services", "integrations"),
        ("pipeline", "downloads"), ("releases", "downloads"), ("streamrip", "downloads"), ("torznab", "downloads"),
        ("slskd", "downloads"), ("ytdlp", "downloads"), ("youtube", "downloads"), ("library", "library"),
        ("verify", "library"), ("translations", "library"), ("spotify", "integrations"), ("media", "integrations"), ("roon", "integrations"),
        ("cadenza", "processing"), ("general", "general"), ("email", "accounts"), ("sync", "tasks"),
    ] {
        assert_eq!(page_of(link), Some(page), "?s={link}");
    }
    // no page has headings over its cards any more (each page is one subject)
    assert!(SCHEMA.iter().flat_map(|s| s.groups.iter()).all(|g| g.part.is_empty()));
}

/// web-rs draws its settings tests from a saved schema answer (web-rs/src/settings/fixtures/settings-schema.json,
/// from a server with ENCORE_LIBRARY_PATH set): it has to be this table's. `ENCORE_WRITE_SETTINGS_SCHEMA=1 cargo test
/// -p encore-config schema_fixture` writes it again. A checkout without web-rs skips the comparison.
#[test]
fn the_web_schema_fixture_is_this_table() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../web-rs/src/settings/fixtures/settings-schema.json");
    let d = encore_core::tmp::dir("settings-fixture");
    let cfg = Config::load_with(Db::open(&d).unwrap(), |name| (name == "ENCORE_LIBRARY_PATH").then(|| "/music".to_string())).unwrap();
    let schema = cfg.schema();
    let _ = std::fs::remove_dir_all(d);
    if std::env::var_os("ENCORE_WRITE_SETTINGS_SCHEMA").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&schema).unwrap() + "\n").expect("write the schema fixture");
    }
    let Ok(saved) = std::fs::read_to_string(&path) else { return };
    let saved: Value = serde_json::from_str(&saved).expect("the fixture is JSON");
    assert!(saved == schema, "{} is not this table's schema: write it again (see above)", path.display());
}

/// Every key the server's code reads through `cfg.str("…")` and its siblings is in the table: a misspelt key
/// would read as empty.
#[test]
fn every_key_the_code_reads_is_in_the_table() {
    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // `st.cfg.get(…)` and `cfg().get(…)` are the settings; a local `cfg.get(…)` can be another map (streamrip's file)
    let re = regex::Regex::new(r#"(?:(?:\.cfg|\bcfg\(\))\.(?:str|bool|int|float|get|env_locked|def)|\bcfg\.(?:str|bool|int|float|env_locked))\(\s*"([a-z0-9_]+)""#).unwrap();
    let mut files = vec![];
    let mut dirs = vec![server.join("src"), server.join("crates")];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    let mut found = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for c in re.captures_iter(&text) {
            let key = &c[1];
            // test helpers make up keys on purpose
            if key.starts_with("no_such") {
                continue;
            }
            assert!(def(key).is_some(), "{}: {key} is not in settings.toml", f.display());
            found += 1;
        }
    }
    assert!(found > 100, "the scan found the settings reads ({found})");
}
