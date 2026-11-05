//! /api/admin/lantern through the router: admins only; saving checks the address and never answers the key;
//! the dry run lists what the move would carry with secrets masked; the move itself waits for Lantern 1.0
//! (501, nothing changed); the plugin cards are marked once Lantern has taken over; disconnecting clears it.
use crate::test_support::{call, send, user, TestServer};
use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn the_lantern_card_connects_dry_runs_and_waits_for_the_move() {
    let ts = TestServer::new("lantern-card").with_sources().with(json!({
        "torznab_enabled": true, "prowlarr_url": "http://prowlarr:9696", "prowlarr_api_key": "prowlarr-secret",
    }));
    user(&ts.st, "boss-lantern", "admin");
    user(&ts.st, "plain-lantern", "user");
    let app = ts.app();
    let login = |name: &'static str| {
        let app = app.clone();
        async move {
            let (code, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
            assert_eq!(code, StatusCode::OK);
            cookie.split(';').next().unwrap_or("").to_string()
        }
    };
    let (admin, plain) = (login("boss-lantern").await, login("plain-lantern").await);
    assert_eq!(send(&app, "GET", "/api/admin/lantern", &plain, None, &[]).await.status, StatusCode::FORBIDDEN);

    // not connected yet; the move needs a connection
    let r = send(&app, "GET", "/api/admin/lantern", &admin, None, &[]).await;
    assert_eq!((r.status, r.json()["connected"].clone()), (StatusCode::OK, json!(false)));
    let r = send(&app, "POST", "/api/admin/lantern/migrate", &admin, Some(json!({ "dry_run": false })), &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());

    // a bad address, then a good one; the key never comes back
    let r = send(&app, "PUT", "/api/admin/lantern", &admin, Some(json!({ "url": "lantern:8080", "api_key": "k" })), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(&app, "PUT", "/api/admin/lantern", &admin, Some(json!({ "url": "http://lantern:8080/", "api_key": "lantern-secret" })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let s = r.json();
    assert_eq!((s["url"].clone(), s["connected"].clone(), s["took_over"].clone()), (json!("http://lantern:8080"), json!(true), json!(false)));
    assert!(!r.text().contains("lantern-secret"));

    // the dry run: Prowlarr and the client, no secret; Soulseek is off
    let r = send(&app, "POST", "/api/admin/lantern/migrate", &admin, Some(json!({})), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let d = r.json();
    assert_eq!(d["dry_run"], json!(true));
    let items = d["plan"]["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["key"] == "prowlarr_url" && i["value"] == "http://prowlarr:9696"));
    assert!(items.iter().any(|i| i["key"] == "qbit_url"));
    assert!(!r.text().contains("prowlarr-secret"));
    assert!(d["plan"]["skipped"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("Soulseek")));

    // the move waits for Lantern 1.0 and changes nothing
    let r = send(&app, "POST", "/api/admin/lantern/migrate", &admin, Some(json!({ "dry_run": false })), &[]).await;
    assert_eq!(r.status, StatusCode::NOT_IMPLEMENTED, "{}", r.text());
    assert!(!ts.st.cfg.bool("lantern_took_over"));
    let r = send(&app, "GET", "/api/plugins", &admin, None, &[]).await;
    assert!(r.json()["plugins"].as_array().unwrap().iter().all(|c| c["handled_by"].is_null()));

    // once Lantern has taken over (what the move will set), its cards are marked; others aren't
    ts.st.cfg.set_many(&json!({ "lantern_took_over": true }));
    let cards = send(&app, "GET", "/api/plugins", &admin, None, &[]).await.json();
    let handled = |id: &str| cards["plugins"].as_array().unwrap().iter().find(|c| c["id"] == id).map(|c| c["handled_by"].clone());
    assert_eq!(handled("torznab"), Some(json!("lantern")));
    assert_eq!(handled("slskd"), Some(json!("lantern")));
    assert_eq!(handled("qbittorrent"), Some(json!("lantern")));
    assert_eq!(handled("ytdlp"), Some(serde_json::Value::Null));
    assert_eq!(send(&app, "GET", "/api/plugins/slskd", &admin, None, &[]).await.json()["handled_by"], json!("lantern"));

    // disconnecting brings Encore's own back, the native settings as they were
    let r = send(&app, "DELETE", "/api/admin/lantern", &admin, None, &[]).await;
    assert_eq!((r.status, r.json()["connected"].clone()), (StatusCode::OK, json!(false)));
    assert!(!ts.st.cfg.bool("lantern_took_over"));
    assert_eq!(ts.st.cfg.str("prowlarr_url"), "http://prowlarr:9696");

    // the kit's test route knows Lantern
    let r = send(&app, "POST", "/api/system/test/lantern", &admin, Some(json!({})), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["ok"], json!(false));
}
