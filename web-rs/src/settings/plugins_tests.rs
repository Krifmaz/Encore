//! The Plugins page (plugins.rs): what each kind of plugin card shows, the source cards that moved from
//! Downloads drawn as they were, the old `#/settings?s=` links, and the words of actions and failures.
//!
//! The plugin answers are made the way the server makes them (crates/plugins registry.rs `card_of`,
//! `settings`): a built-in plugin's fields are the settings schema's fields of its keys (fixtures/
//! settings-schema.json) with `part`, `store_key`, `env_locked`.

use super::form_tests::{cases, schema, schema_json};
use super::groups::page_cards;
use super::plugins::*;
use super::schema_form::form_html;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The schema field of `key`, as JSON.
fn schema_field(key: &str) -> Value {
    schema_json()["sections"].as_array().unwrap().iter()
        .flat_map(|s| s["groups"].as_array().unwrap().iter())
        .flat_map(|g| g["fields"].as_array().unwrap().iter())
        .find(|f| f["key"] == key).cloned().unwrap_or_else(|| panic!("no field {key} in the schema fixture"))
}

/// GET /api/plugins/{id}/settings of a built-in plugin with these keys (`part` on the first key of a part).
fn builtin_answer(id: &str, keys: &[(&str, Option<&str>)]) -> Value {
    let fields: Vec<Value> = keys.iter().map(|(k, part)| {
        let mut f = schema_field(k);
        f["part"] = json!(part);
        f["store_key"] = json!(k);
        f
    }).collect();
    let values: serde_json::Map<String, Value> = keys.iter().map(|(k, _)| (k.to_string(), schema_field(k)["default"].clone())).collect();
    json!({ "id": id, "builtin": true, "settings_on": null, "fields": fields, "values": values })
}

/// A card of GET /api/plugins.
fn card(id: &str, installed: bool) -> Value {
    json!({ "id": id, "name": id, "description": format!("What {id} does"), "kind": "source", "version": "0.32.0",
        "installed": installed, "installed_version": if installed { json!("1.2.3") } else { Value::Null }, "enabled": installed,
        "origin": "catalog", "trust": "first-party", "builtin": true, "warning": null, "auto_update": true, "can_update": true,
        "program": true, "latest": null, "update_available": false, "last_check": null, "last_error": null, "status": null, "busy": null,
        "sandbox": "Full isolation", "network": true, "hosts": [], "reads_library": false, "commands": ["status", "test"],
        "kinds": ["track", "album"], "lossless": false, "needs_sign_in": false, "sources": [], "settings_on": null,
        "has_settings": true, "homepage": "https://example.org", "license": "MIT", "min_encore": null, "migration": null })
}

fn upload_card() -> Value {
    let mut c = card("internet-archive", true);
    c["name"] = json!("Internet Archive <b>");
    c["origin"] = json!("upload");
    c["trust"] = json!("third-party");
    c["builtin"] = json!(false);
    c["warning"] = json!("Third-party plugin: not made by Encore. It runs in full isolation: it sees only its own folders, and the network only when its plugin.toml asks for it (the line below says what it can reach). Install only plugins you trust.");
    c["hosts"] = json!(["archive.org"]);
    c["homepage"] = json!("javascript:alert(1)");
    c
}

#[test]
fn the_moved_cards_and_old_links_open_the_plugins_page() {
    let schema = schema();
    for (s, page) in [
        ("streamrip", "plugins"), ("torznab", "plugins"), ("slskd", "plugins"), ("ytdlp", "plugins"), ("youtube", "plugins"),
        ("tools", "plugins"), ("plugins", "plugins"), ("qbittorrent", "plugins"), ("translator", "plugins"),
        // what stays where it was
        ("pipeline", "downloads"), ("releases", "downloads"), ("apps", "downloads"), ("downloads", "downloads"),
        ("cadenza", "processing"), ("media", "integrations"), ("account", "account"),
    ] {
        assert_eq!(super::page_for(&schema, s), page, "?s={s}");
    }
    assert_eq!(card_for("youtube"), "ytdlp");
    assert_eq!(card_for("slskd"), "slskd");
    assert_eq!(card_for("qbittorrent"), "qbittorrent");
    // the nav: Plugins right after Downloads
    let nav: Vec<&str> = super::server_nav(&schema).iter().map(|(id, _)| *id).collect();
    let at = nav.iter().position(|id| *id == "downloads").unwrap();
    assert_eq!(nav[at + 1], "plugins");
    assert_eq!(nav.iter().filter(|id| **id == "plugins").count(), 1);
}

#[test]
fn downloads_keeps_the_pipeline_and_release_types() {
    let schema = schema();
    for (case, values) in cases() {
        let page = schema.section("downloads").unwrap();
        let ids: Vec<&str> = page_cards(&page.groups, &values).iter().map(|(g, _)| g.id.as_str()).collect();
        assert_eq!(ids, ["pipeline", "releases"], "{case}");
        // the other pages keep every card
        for other in schema.sections.iter().filter(|s| s.id != "downloads") {
            assert_eq!(page_cards(&other.groups, &values).len(), super::groups::visible_cards(&other.groups, &values).len(), "{}", other.id);
        }
    }
}

#[test]
fn a_moved_card_draws_its_fields_as_the_downloads_page_did() {
    let schema = schema();
    for (_, values) in cases() {
        for reveal in [false, true] {
            for id in ["streamrip", "slskd"] {
                let g = schema.group(id).unwrap();
                let keys: Vec<(&str, Option<&str>)> = g.fields.iter().map(|f| (f.key.as_str(), None)).collect();
                let answer = builtin_answer(id, &keys);
                let fields: Vec<&super::fields::Field> = g.fields.iter().collect();
                let before = form_html(&fields, &values, reveal);
                let after = form_body_html(&answer, &card_values(&answer, &values), reveal, &schema);
                assert_eq!(after, before, "{id}, reveal {reveal}");
            }
            // yt-dlp: its own fields, then YouTube sign-in as a part with the card's words, block and Test
            let yt = schema.group("youtube").unwrap();
            let own = schema.group("ytdlp").unwrap();
            let mut keys: Vec<(&str, Option<&str>)> = own.fields.iter().map(|f| (f.key.as_str(), None)).collect();
            keys.extend(yt.fields.iter().enumerate().map(|(i, f)| (f.key.as_str(), if i == 0 { Some("YouTube sign-in") } else { None })));
            let answer = builtin_answer("ytdlp", &keys);
            let html = form_body_html(&answer, &card_values(&answer, &values), reveal, &schema);
            let own_fields: Vec<&super::fields::Field> = own.fields.iter().collect();
            let yt_fields: Vec<&super::fields::Field> = yt.fields.iter().collect();
            assert!(html.starts_with(&form_html(&own_fields, &values, reveal)), "{html}");
            assert!(html.ends_with(&form_html(&yt_fields, &values, reveal)), "{html}");
            assert!(html.contains(r#"<div data-part="YouTube sign-in""#));
            assert!(html.contains(&crate::ui::esc(&yt.desc)), "the YouTube sign-in card's words");
            assert!(html.contains(r#"data-part-test="youtube""#) && html.contains("Test cookies"));
            assert!(html.contains(&crate::pages::youtube::youtube_block()));
        }
    }
    // the blocks under the fields: Tidal and the streamrip config, the Soulseek status
    assert!(extras_html("streamrip").contains(r#"id="tidal-login""#) && extras_html("streamrip").contains(r#"id="rip-config""#));
    assert!(extras_html("slskd").contains(r#"id="slsk-engine""#));
    assert_eq!(extras_html("torznab"), "");
}

#[test]
fn a_client_card_is_shown_whole_with_the_choice_that_uses_it() {
    let schema = schema();
    let keys = [("qbit_url", None), ("qbit_user", None), ("qbit_password", None), ("qbit_category", None), ("qbit_download_path", None)];
    let answer = builtin_answer("qbittorrent", &keys);
    // the fields name torznab's select: on the client's own card the rule is left out
    assert!(schema_field("qbit_url")["when"].as_str().unwrap().starts_with("torrent_client="));
    let g = group_of(&answer, "qBittorrent");
    assert!(g.fields.iter().all(|f| f.when.is_none()));
    let mut values = cases()[0].1.clone();
    values["torrent_client"] = json!("deluge");
    let html = form_body_html(&answer, &card_values(&answer, &values), false, &schema);
    assert!(!html.contains("data-when"), "{html}");
    assert!(!html.contains(" hidden class=\"field"), "no field starts hidden");
    // the note says whether it is the chosen client
    let mut c = card("qbittorrent", true);
    c["kind"] = json!("client");
    c["name"] = json!("qBittorrent");
    let list = json!({ "plugins": [c.clone()] });
    let body = body_html(&c, Some(&answer), &values, false, &schema, &list);
    assert!(body.contains("Not in use: Torrent client is set to Deluge on the Torrents &amp; Usenet card. Choose qBittorrent there to send downloads to qBittorrent."), "{body}");
    values["torrent_client"] = json!("qbittorrent");
    let body = body_html(&c, Some(&answer), &values, false, &schema, &list);
    assert!(body.contains("In use: Torrent client is set to qBittorrent on the Torrents &amp; Usenet card."), "{body}");
    // a rule between two fields of the same card stays
    let mut a2 = answer.clone();
    a2["fields"][1]["when"] = json!("qbit_category=x");
    let g = group_of(&a2, "qBittorrent");
    assert!(g.fields[1].when.is_some() && g.fields[0].when.is_none());
}

#[test]
fn the_torrents_card_names_a_chosen_client_whose_plugin_is_missing() {
    let schema = schema();
    let keys = [("torznab_enabled", None), ("prowlarr_url", None), ("torrent_client", None), ("nzb_client", None)];
    let answer = builtin_answer("torznab", &keys);
    let mut values = cases()[0].1.clone();
    values["torrent_client"] = json!("blackhole");
    values["nzb_client"] = json!("sabnzbd");
    let mut hole = card("torrent-blackhole", false);
    hole["kind"] = json!("client");
    let mut sab = card("sabnzbd", true);
    sab["kind"] = json!("client");
    sab["enabled"] = json!(false);
    let list = json!({ "plugins": [card("torznab", true), hole, sab] });
    let body = body_html(&card("torznab", true), Some(&answer), &values, false, &schema, &list);
    assert!(body.contains(r#"data-act="install" data-id="torrent-blackhole""#), "{body}");
    assert!(body.contains("Torrent client: Blackhole folder (any client with a watch folder) has a plugin of its own, not installed yet."), "{body}");
    assert!(body.contains(r#"data-act="enable" data-id="sabnzbd""#) && body.contains("Usenet client: the SABnzbd plugin is disabled."), "{body}");
    // every choice of the two selects has its client plugin
    for key in ["torrent_client", "nzb_client"] {
        let f = schema.group("torznab").unwrap().field(key).cloned().unwrap();
        for (v, _) in &f.options { assert!(CLIENTS.iter().any(|(k, val, _)| *k == key && val == v), "{key}={v}"); }
    }
}

#[test]
fn installed_available_and_third_party_cards() {
    let acting = HashMap::new();
    // installed, first-party: the buttons, the version of the program, the auto-update switch
    let mut c = card("ytdlp", true);
    c["name"] = json!("yt-dlp");
    c["installed_version"] = json!("2026.10.14");
    c["status"] = json!({ "ok": true, "message": "yt-dlp 2026.10.14 (installed by the plugin)" });
    let html = installed_card_html(&c, "", true, None, "Test");
    for want in [r#"data-plugin="ytdlp" open"#, "2026.10.14", r#"data-act="update""#, r#"data-act="test""#, r#"data-act="logs""#,
        r#"data-act="disable""#, r#"data-act="remove""#, "data-auto", "Update automatically (recommended – sites change often)",
        "yt-dlp 2026.10.14 (installed by the plugin)", "data-logs hidden", "Finds songs and albums", "dot ok"] {
        assert!(html.contains(want), "{want}: {html}");
    }
    assert!(!html.contains("Third-party"));
    // an update found
    c["latest"] = json!("2026.11.01");
    c["update_available"] = json!(true);
    let html = installed_card_html(&c, "", false, None, "Test");
    assert!(html.contains("Update to 2026.11.01") && html.contains("2026.11.01 available"), "{html}");
    // switched off: Enable, and the line says so
    c["enabled"] = json!(false);
    let html = installed_card_html(&c, "", false, None, "Test");
    assert!(html.contains(r#"data-act="enable""#) && html.contains("Disabled – Encore does not use it until you enable it again") && html.contains(r#"<span class="badge">Off</span>"#));
    // while the page installs or the server is busy: every button waits
    let html = actions_html(&c, Some("update"), "Test");
    assert!(html.contains("Updating…") && !html.contains(r#"data-act="test" >"#));
    assert!(html.matches("disabled").count() >= 5, "{html}");
    c["busy"] = json!("updating");
    assert!(status_html(&c, None).contains("Updating…"));
    // not installed: a row with Install
    let row = available_html(&card("slskd", false), None);
    assert!(row.contains(r#"data-plugin-row="slskd""#) && row.contains(r#"data-act="install""#) && !row.contains(r#"data-act="remove""#), "{row}");
    // third-party: the warning, the sandbox and network line, the plugin's own text escaped, no unsafe links
    let up = upload_card();
    let html = installed_card_html(&up, "", false, None, "Test");
    assert!(html.contains("Third-party plugin: not made by Encore.") && html.contains(r#"<span class="badge warn">Third-party</span>"#));
    assert!(html.contains("Sandbox: Full isolation · Network: archive.org"), "{html}");
    assert!(html.contains("Internet Archive &lt;b&gt;") && !html.contains("Internet Archive <b>"));
    assert!(!html.contains("javascript:"), "{html}");
    // the whole page: the head, the cards, the list, the add form
    let list = json!({ "sandbox": { "level": "limits", "text": "Limits only, see the compose option", "reason": "bwrap: No permissions to create new namespace" },
        "upload_max_mb": 200, "plugins": [card("ytdlp", true), card("slskd", false), up] });
    let page = page_html(&list, |_| String::new(), &[], &acting, |_| "Test".into());
    for want in ["Check for updates", "Limits only, see the compose option", "bwrap: No permissions to create new namespace", "Installed", "Available",
        "Upload plugin", "Install from URL", "owner/repo@ref", "A .zip of at most 200 MB", r#"id="plugin-file""#] {
        assert!(page.contains(want), "{want}");
    }
    assert!(page.find(r#"data-plugin="ytdlp""#).unwrap() < page.find(r#"data-plugin-row="slskd""#).unwrap());
    // nothing installed yet
    let fresh = json!({ "sandbox": { "level": "full", "text": "Full isolation" }, "plugins": [card("ytdlp", false)] });
    let page = page_html(&fresh, |_| String::new(), &[], &acting, |_| "Test".into());
    assert!(page.contains("No plugins installed yet") && page.contains(r#"<span class="badge ok">Full isolation</span>"#));
}

#[test]
fn settings_elsewhere_and_a_plugins_own_settings() {
    let schema = schema();
    let values = cases()[0].1.clone();
    let list = json!({ "plugins": [] });
    // Cadenza's fields stay on Server processing
    let mut c = card("cadenza", true);
    c["settings_on"] = json!("processing");
    assert!(body_html(&c, None, &values, false, &schema, &list).contains(r##"<a href="#/settings?s=processing" style="color:var(--accent)">Settings → Server processing</a>"##));
    // a plugin's own settings: its words, its saved secret shown as saved, never locked, no eye button
    let answer = json!({ "id": "internet-archive", "builtin": false, "settings_on": null,
        "fields": [
            { "key": "format", "label": "Format", "type": "select", "options": [{ "value": "flac", "label": "FLAC" }, { "value": "mp3", "label": "MP3" }], "default": "flac", "secret": false, "env": null, "env_locked": false, "part": null },
            { "key": "token", "label": "Token", "type": "password", "secret": true, "env": null, "env_locked": false, "part": "Account" },
            { "key": "max_items", "label": "Most items", "type": "number", "default": 25, "secret": false, "env": null, "env_locked": false, "part": null },
        ],
        "values": { "format": "mp3", "token": "•••• (saved)", "max_items": 25 } });
    let v = card_values(&answer, &values);
    assert_eq!(v["format"], "mp3");
    assert!(v.get("app_name").is_none(), "Encore's settings are not a plugin's");
    assert_eq!(v["_meta"]["token"]["source"], "ui");
    let html = form_body_html(&answer, &v, true, &schema);
    assert!(html.contains(r#"<option value="mp3" selected>MP3</option>"#), "{html}");
    assert!(html.contains("Saved ✓ (hidden)") && !html.contains("data-reveal"), "{html}");
    assert!(html.contains(r#"<div data-part="Account""#) && html.contains("<h3>Account</h3>"));
    let runs = runs(&answer);
    assert_eq!(runs.iter().map(|(p, f)| (p.as_str(), f.len())).collect::<Vec<_>>(), [("", 1), ("Account", 2)]);
    // a card whose settings could not be read says why, with Copy
    let mut up = upload_card();
    up["has_settings"] = json!(true);
    let body = body_html(&up, Some(&json!({ "_error": "Unknown plugin" })), &values, false, &schema, &list);
    assert!(body.contains("Unknown plugin") && body.contains("copy-btn"));
    // a built-in plugin: Encore's values and their _meta (an env lock stays a lock)
    let answer = builtin_answer("ytdlp", &[("ytdlp_enabled", None)]);
    let mut settings = values.clone();
    settings["_meta"]["ytdlp_enabled"] = json!({ "source": "env", "env": "YTDLP_ENABLED" });
    let html = form_body_html(&answer, &card_values(&answer, &settings), false, &schema);
    assert!(html.contains("Set by <span class=\"mono\">YTDLP_ENABLED</span>"), "{html}");
}

#[test]
fn words_of_actions_tests_and_failures() {
    let mut before = card("ytdlp", true);
    before["name"] = json!("yt-dlp");
    let mut after = before.clone();
    assert_eq!(done_words("update", &before, &after), "yt-dlp is up to date");
    after["installed_version"] = json!("2026.11.01");
    assert_eq!(done_words("update", &before, &after), "yt-dlp updated to 2026.11.01");
    assert_eq!(done_words("install", &card("torznab", false), &json!({ "name": "Torrents & Usenet (Prowlarr)", "installed": true })), "Torrents & Usenet (Prowlarr) installed");
    assert_eq!(done_words("install", &before, &after), "yt-dlp 2026.11.01 installed");
    assert_eq!(done_words("remove", &before, &json!({ "id": "ytdlp", "installed": false, "name": "yt-dlp" })), "yt-dlp removed");
    assert_eq!(done_words("disable", &before, &after), "yt-dlp disabled");
    // Test answers: passed, failed, rate limited
    assert_eq!(test_answer(&json!({ "ok": true, "message": "yt-dlp 2026.10.14 runs" })), (true, "yt-dlp 2026.10.14 runs".into()));
    assert_eq!(test_answer(&json!({ "ok": false, "error": "archive.org did not answer", "code": "network" })), (false, "archive.org did not answer".into()));
    assert_eq!(test_answer(&json!({ "ok": false, "error": "Too many requests", "code": "rate_limited", "retry_after": 90 })).1, "Too many requests – try again in 1 m 30 s");
    assert_eq!(test_answer(&json!({ "ok": true })).1, "Test passed");
    let html = test_html(&json!({ "ok": false, "error": "Sign-in needed", "code": "auth" }));
    assert!(html.contains("test-result err") && html.contains("Sign-in needed") && html.contains("copy-btn"));
    // a refused manifest: the sentence and each problem, all of it copyable
    let text = error_text("The plugin.toml has problems", &json!({ "detail": "The plugin.toml has problems", "problems": ["[plugin] id: missing", "[run] shell: must be false"] }));
    assert_eq!(text, "The plugin.toml has problems\n- [plugin] id: missing\n- [run] shell: must be false");
    let html = error_html(&text);
    assert!(html.contains("white-space:pre-wrap") && html.contains(r#"data-copy-text="The plugin.toml has problems&#10;- [plugin] id: missing"#) || html.contains("data-copy-text=\"The plugin.toml has problems\n- [plugin] id: missing"), "{html}");
    assert_eq!(error_text("Unknown plugin", &Value::Null), "Unknown plugin");
    // what is checked before anything is sent
    assert_eq!(url_problem("  "), Some("Enter an https address, or owner/repo@ref for GitHub"));
    assert_eq!(url_problem("http://example.org/p.zip"), Some("Plugins install only from https addresses"));
    assert_eq!(url_problem("Krifmaz/encore-plugins/internet-archive@v0.1.0"), None);
    assert_eq!(url_problem("https://example.org/plugin.toml"), None);
    assert_eq!(file_problem("ia.zip", 1e6, 200), None);
    assert_eq!(file_problem("ia.tar.gz", 1e6, 200).unwrap(), "A plugin is uploaded as a .zip file");
    assert_eq!(file_problem("big.ZIP", 201.0 * 1024.0 * 1024.0, 200).unwrap(), "The plugin zip is larger than 200 MB");
    // the warning before a plugin of someone else's is installed, for each sandbox level
    assert!(trust_text(true).contains("full isolation") && trust_text(false).contains("can read what Encore can"));
    assert!(trust_text(true).ends_with("Install only plugins you trust.") && trust_text(false).ends_with("Install only plugins you trust."));
    assert_eq!(kind_label("client"), "Download client");
    assert_eq!(kind_label("widget"), "Widget");
    assert_eq!(kinds_words(&[json!("track"), json!("album"), json!("url")]), "songs, albums and links");
}
