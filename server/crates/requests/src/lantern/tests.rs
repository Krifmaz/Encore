//! Lantern: the connection's two steps, the routing of the sources, the dry run of the move and the
//! connection test against a stand-in Lantern.
use super::*;
use crate::test_ctx::TestCtx;

/// `v` as owned strings.
fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Serve `app` on a free local port; its base address.
async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

#[test]
fn lantern_takes_the_place_of_the_first_source_it_replaces() {
    // the quality-first order: FLAC on Soulseek, the indexers, then Soulseek in any format – all one Lantern turn
    let order = names(&["streamrip", "slskd_lossless", "torznab", "slskd", "ytdlp"]);
    assert_eq!(route(order), names(&["streamrip", "lantern", "ytdlp"]));
    // the admin's own order is kept around it
    assert_eq!(route(names(&["torznab", "streamrip", "slskd", "zotify"])), names(&["lantern", "streamrip", "zotify"]));
    // nothing Lantern replaces (a hi-res upgrade asks streamrip only): no Lantern turn
    assert_eq!(route(names(&["streamrip"])), names(&["streamrip"]));
    assert_eq!(route(vec![]), Vec::<String>::new());
}

#[test]
fn addresses_are_checked_and_trimmed() {
    assert_eq!(base("  http://lantern:8080/ \n"), "http://lantern:8080");
    assert_eq!(url_problem("http://lantern:8080"), None);
    assert_eq!(url_problem("https://lantern.example.org/"), None);
    assert!(url_problem("lantern:8080").is_some());
    assert!(url_problem("http://").is_some());
    assert!(url_problem("").is_some());
}

#[test]
fn connecting_alone_routes_nothing_and_disconnecting_undoes_the_takeover() {
    let st = TestCtx::new("lantern-steps");
    assert!(!connected(&st) && !took_over(&st));
    assert_eq!(save(&st, "ftp://x", Some("k")), Err("Lantern's address starts with http:// or https://"));
    save(&st, "http://lantern:8080/", Some(" key ")).unwrap();
    assert!(connected(&st));
    assert_eq!(Connection::of(&st), Some(Connection { url: "http://lantern:8080".into(), api_key: "key".into() }));
    // connected, but the move isn't done: Encore's own sources and cards stay
    assert!(!took_over(&st) && !hides_plugin(&st, "slskd"));
    // the masked key the card sends back keeps the saved one
    save(&st, "http://lantern:9090", Some(encore_config::MASK)).unwrap();
    assert_eq!(st.cfg.str("lantern_api_key"), "key");
    let s = state(&st);
    assert_eq!((s["url"].as_str(), s["key_set"].as_bool(), s["api_ready"].as_bool()), (Some("http://lantern:9090"), Some(true), Some(false)));
    assert!(s.to_string().find("\"key\"").is_none(), "the key is never in the state");
    // taken over (what the move sets): the covered cards hide, the others don't
    st.cfg.set_many(&json!({ "lantern_took_over": true }));
    assert!(took_over(&st));
    assert!(COVERED_PLUGINS.iter().all(|p| hides_plugin(&st, p)));
    assert!(!hides_plugin(&st, "streamrip") && !hides_plugin(&st, "ytdlp"));
    // a key that went away: not taken over any more, without a word from anyone
    st.cfg.set_many(&json!({ "lantern_api_key": "" }));
    assert!(!took_over(&st));
    st.cfg.set_many(&json!({ "lantern_api_key": "key" }));
    disconnect(&st);
    assert!(!connected(&st) && !st.cfg.bool("lantern_took_over"));
}

#[test]
fn the_dry_run_lists_the_chosen_clients_and_soulseek_with_secrets_masked() {
    let st = TestCtx::new("lantern-plan");
    // both cards off: nothing to move, and the reasons
    let p = migrate::plan(&st.cfg);
    assert!(p.is_empty());
    assert_eq!(p.skipped.len(), 2);
    st.cfg.set_many(&json!({
        "torznab_enabled": true, "prowlarr_url": "http://prowlarr:9696", "prowlarr_api_key": "secret-p",
        "torrent_client": "transmission", "transmission_url": "http://tr:9091", "nzb_client": "sabnzbd",
        "sab_url": "http://sab:8080", "sab_api_key": "secret-s",
    }));
    let p = migrate::plan(&st.cfg);
    let keys: Vec<&str> = p.items.iter().map(|i| i.key).collect();
    // the chosen clients' fields only
    assert!(keys.contains(&"transmission_url") && keys.contains(&"sab_url") && keys.contains(&"prowlarr_url"));
    assert!(!keys.iter().any(|k| k.starts_with("qbit_") || k.starts_with("nzbget_") || k.starts_with("deluge_")));
    let item = |k: &str| p.items.iter().find(|i| i.key == k).unwrap();
    assert_eq!(item("prowlarr_url").part, "Indexers (Prowlarr)");
    assert_eq!(item("torrent_client").part, "Torrent client");
    assert_eq!(item("transmission_url").part, "Torrent client");
    assert_eq!(item("sab_api_key").part, "Usenet client");
    // secrets: only whether they are set
    assert_eq!(item("prowlarr_api_key").value, json!(encore_config::MASK));
    assert!(!serde_json::to_string(&p).unwrap().contains("secret-"));
    assert!(item("torznab_min_seeders").default && !item("prowlarr_url").default);
    // Soulseek is still off: said so
    assert!(p.skipped.iter().any(|s| s.contains("Soulseek")), "{:?}", p.skipped);
    st.cfg.set_many(&json!({ "slskd_enabled": true, "soulseek_username": "me" }));
    let p = migrate::plan(&st.cfg);
    assert!(p.skipped.is_empty());
    assert_eq!(p.items.iter().find(|i| i.key == "soulseek_username").map(|i| i.part), Some("Soulseek"));
    assert_eq!(p.items.iter().find(|i| i.key == "soulseek_password").map(|i| i.value.clone()), Some(json!("")));
}

#[test]
fn the_move_waits_for_lantern_1_0_and_changes_nothing() {
    let st = TestCtx::new("lantern-apply");
    save(&st, "http://lantern:8080", Some("k")).unwrap();
    assert_eq!(migrate::apply(), Err(NOT_YET));
    assert!(!took_over(&st));
}

#[tokio::test]
async fn the_test_needs_a_connection_and_an_answer() {
    let st = TestCtx::new("lantern-test");
    assert_eq!(test(&st).await["ok"], json!(false));
    let url = serve(axum::Router::new().route("/", axum::routing::get(|| async { "lantern" }))).await;
    save(&st, &url, Some("k")).unwrap();
    let r = test(&st).await;
    assert_eq!(r["ok"], json!(true), "{r}");
    assert!(r["message"].as_str().unwrap().contains("HTTP 200"));
    // nothing listening there
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    save(&st, &dead, None).unwrap();
    let r = test(&st).await;
    assert_eq!(r["ok"], json!(false), "{r}");
    assert!(r["message"].as_str().unwrap().contains("isn't reachable"));
}
