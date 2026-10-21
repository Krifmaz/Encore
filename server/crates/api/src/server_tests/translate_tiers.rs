//! The translation tiers on a whole server (encore_library::translate::tiers): the order of the sources, a
//! stronger one replacing a weaker one's text on rows made before it, DeepL's client against a stand-in on
//! 127.0.0.1 (batches of 50, the key, the free and pro plans' addresses), its monthly budget, and the states
//! Settings → Language shows.
use crate::test_support::{call, user, TestServer};
use axum::extract::State as S;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use encore_db::{p, RowExt};
use encore_library::translate::prompt::Kind;
use encore_library::translate::{deepl, pass_now, store, store::Source};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// What the stand-in for DeepL was sent: (Authorization header, body) per request.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// DeepL's /v2/translate on 127.0.0.1: "EN:" before each text, 403 for the key "bad", 456 for the key "spent", an
/// answer without the translations for the key "lost" (taken and counted by DeepL, the answer lost on the way).
async fn stand_in() -> (String, Seen) {
    async fn translate(S(seen): S<Seen>, headers: HeaderMap, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        seen.lock().unwrap().push((auth.clone(), body.clone()));
        match auth.as_str() {
            "DeepL-Auth-Key bad" => return (StatusCode::FORBIDDEN, Json(json!({ "message": "Wrong key" }))),
            "DeepL-Auth-Key spent" => return (StatusCode::from_u16(456).unwrap(), Json(json!({ "message": "Quota exceeded" }))),
            "DeepL-Auth-Key lost" => return (StatusCode::OK, Json(json!({}))),
            _ => {}
        }
        let out: Vec<Value> = body["text"].as_array().unwrap().iter()
            .map(|t| json!({ "detected_source_language": "JA", "text": format!("EN:{}", t.as_str().unwrap()) })).collect();
        (StatusCode::OK, Json(json!({ "translations": out })))
    }
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let app = Router::new().route("/v2/translate", post(translate)).with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

fn server(name: &str, deepl_url: &str, settings: Value) -> TestServer {
    TestServer::new(name).with_urls(encore_http::Urls { deepl: deepl_url.to_string(), ..Default::default() }).with(settings)
}

fn shown(st: &crate::state::AppState, text: &str) -> store::Shown {
    store::lookup(&st.db, "en", &[text.to_string()]).remove(text).unwrap_or_default()
}

fn sent_texts(seen: &Seen) -> Vec<String> {
    seen.lock().unwrap().iter().flat_map(|(_, b)| b["text"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect::<Vec<_>>()).collect()
}

/// The DeepL client: titles go in requests of at most 50 with the key in the header, the target language as
/// DeepL names it and the source language where the script tells it; the answers are stored with the source
/// `deepl` and the characters counted for the month. Artist names are never sent.
#[tokio::test]
async fn deepl_batches_of_fifty_with_the_key_and_the_count() {
    let (base, seen) = stand_in().await;
    let ts = server("deepl-batches", &base, json!({ "deepl_api_key": "k-123:fx" }));
    let st = ts.st.clone();
    let titles: Vec<String> = (0..120).map(|i| format!("ハルジオン{i}")).collect();
    let mut items: Vec<(String, Kind)> = titles.iter().map(|t| (t.clone(), Kind::Title)).collect();
    items.push(("ヨルシカ".into(), Kind::Artist));
    items.push(("Кино".into(), Kind::Album));
    let n = deepl::pass(&st, "en", &items).await.unwrap();
    assert_eq!(n, 121);
    let reqs = seen.lock().unwrap().clone();
    // the Cyrillic album on its own (another source language), then the Japanese titles in 50 + 50 + 20
    let sizes: Vec<usize> = reqs.iter().map(|(_, b)| b["text"].as_array().unwrap().len()).collect();
    assert_eq!(sizes, vec![1, 50, 50, 20]);
    for (auth, body) in &reqs {
        assert_eq!(auth, "DeepL-Auth-Key k-123:fx");
        assert_eq!(body["target_lang"], "EN-US");
        assert_eq!(body["split_sentences"], "0");
    }
    assert_eq!(reqs[1].1["source_lang"], "JA");
    assert!(reqs[0].1.get("source_lang").is_none(), "Cyrillic is several languages: DeepL tells them apart");
    assert!(!sent_texts(&seen).contains(&"ヨルシカ".to_string()), "artist names are not sent");
    let s = shown(&st, "ハルジオン7");
    assert_eq!((s.translation.as_deref(), s.source.as_str()), (Some("EN:ハルジオン7"), "deepl"));
    let r = st.db.one("SELECT deepl_at FROM translations WHERE text='ハルジオン7'", p![]).unwrap().unwrap();
    assert!(r.f("deepl_at") > 0.0);
    let chars: i64 = titles.iter().map(|t| t.chars().count() as i64).sum::<i64>() + 4;
    assert_eq!(deepl::used(&st.db, &deepl::month(encore_db::now())), chars);
    ts.remove();
}

/// The month's budget: DeepL gets what still fits, then pauses until the month is over or the budget is
/// raised; a refused key pauses it for a day or until the key changes (another setting saved leaves the pause).
/// Settings → Language says so.
#[tokio::test]
async fn deepl_stops_at_the_budget_and_after_a_refused_key() {
    let (base, seen) = stand_in().await;
    let ts = server("deepl-budget", &base, json!({ "deepl_api_key": "k-123", "deepl_char_budget": 12 }));
    let st = ts.st.clone();
    // 5 characters each: two fit in 12
    let items: Vec<(String, Kind)> = ["ハルジオA", "ハルジオB", "ハルジオC"].iter().map(|t| (t.to_string(), Kind::Title)).collect();
    assert_eq!(deepl::pass(&st, "en", &items).await, Err(deepl::Error::Budget));
    assert_eq!(sent_texts(&seen), ["ハルジオA", "ハルジオB"]);
    assert_eq!(deepl::used(&st.db, &deepl::month(encore_db::now())), 10);
    assert!(shown(&st, "ハルジオC").translation.is_none());
    // paused: nothing more is sent this month
    assert_eq!(deepl::pass(&st, "en", &items[2..]).await, Ok(0));
    assert_eq!(sent_texts(&seen).len(), 2);
    user(&st, "boss", "admin");
    let app = ts.app();
    let (code, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "boss", "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    let cookie = cookie.split(';').next().unwrap_or("").to_string();
    let (_, s, _) = call(&app, "GET", "/api/translate/status", &cookie, None).await;
    let line = |s: &Value, id: &str| s["tiers"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["state"].as_str().unwrap().to_string();
    assert_eq!(line(&s, "deepl"), "DeepL: 10 of 12 characters this month – paused: this month's DeepL budget is used up");
    // a higher budget lifts the pause
    st.cfg.set_many(&json!({ "deepl_char_budget": 400000 }));
    assert_eq!(deepl::pass(&st, "en", &items[2..]).await, Ok(1));
    let (_, s, _) = call(&app, "GET", "/api/translate/status", &cookie, None).await;
    assert_eq!(line(&s, "deepl"), "DeepL: 15 of 400\u{a0}000 characters this month");
    // a refused key: paused, and the line says why, until the key changes
    st.cfg.set_many(&json!({ "deepl_api_key": "bad" }));
    let more = vec![("マリーゴールド".to_string(), Kind::Title)];
    assert_eq!(deepl::pass(&st, "en", &more).await, Err(deepl::Error::Key));
    let before = seen.lock().unwrap().len();
    assert_eq!(deepl::pass(&st, "en", &more).await, Ok(0), "not asked again with the same key");
    assert_eq!(seen.lock().unwrap().len(), before);
    let (_, s, _) = call(&app, "GET", "/api/translate/status", &cookie, None).await;
    assert!(line(&s, "deepl").contains("the API key was refused"), "{}", line(&s, "deepl"));
    // saving a setting that isn't DeepL's leaves the pause as it is
    st.cfg.set_many(&json!({ "group_listen_guests": true }));
    assert_eq!(deepl::pass(&st, "en", &more).await, Ok(0), "another setting does not lift the pause");
    assert_eq!(seen.lock().unwrap().len(), before);
    st.cfg.set_many(&json!({ "deepl_api_key": "spent" }));
    assert_eq!(deepl::pass(&st, "en", &more).await, Err(deepl::Error::Quota));
    let month = deepl::month(encore_db::now());
    assert_eq!(deepl::used(&st.db, &month), 15, "a refused request is not counted");
    // an answer lost on the way back: DeepL took (and counted) the characters, so they are counted here too
    st.cfg.set_many(&json!({ "deepl_api_key": "lost" }));
    assert!(matches!(deepl::pass(&st, "en", &more).await, Err(deepl::Error::Net(_))));
    assert_eq!(deepl::used(&st.db, &month), 22);
    st.cfg.set_many(&json!({ "deepl_api_key": "good" }));
    assert_eq!(deepl::pass(&st, "en", &more).await, Ok(1));
    assert_eq!(shown(&st, "マリーゴールド").translation.as_deref(), Some("EN:マリーゴールド"));
    assert_eq!(deepl::used(&st.db, &month), 29);
    ts.remove();
}

/// No connection to DeepL (a refused connection, no DNS, the proxy): nothing reached DeepL, so the characters
/// are given back, and DeepL waits a few minutes before it is asked again.
#[tokio::test]
async fn deepl_unreached_counts_nothing() {
    // a port nothing listens on
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let ts = server("deepl-unreached", &closed, json!({ "deepl_api_key": "k-1" }));
    let st = ts.st.clone();
    let items = vec![("ハルジオン".to_string(), Kind::Title)];
    assert!(matches!(deepl::pass(&st, "en", &items).await, Err(deepl::Error::Unreached(_))));
    assert_eq!(deepl::used(&st.db, &deepl::month(encore_db::now())), 0, "nothing was sent, nothing is counted");
    assert!(st.library.translator.deepl_hold(encore_library::translate::tiers::Tiers::read(&st.cfg).deepl_generation()).is_some());
    assert_eq!(deepl::pass(&st, "en", &items).await, Ok(0), "paused for a few minutes");
    ts.remove();
}

/// The tier order on rows made before this version (the model's, the official ones, a correction by hand)
/// and on a new title: the built-in romanization replaces the model's romanization, DeepL the model's
/// translation; official names and corrections stay; nothing is asked twice; a source switched off is left
/// out, and reaches its rows once it is on again.
#[tokio::test]
async fn the_tiers_in_order_and_later() {
    let (base, seen) = stand_in().await;
    let ts = server("tiers-order", &base, json!({ "deepl_api_key": "k-1:fx" }));
    let st = ts.st.clone();
    let u = user(&st, "ann", "user");
    st.db.execute("UPDATE users SET prefs=? WHERE id=?", p![r#"{"translate_mode":"under","translate_lang":"en"}"#, u.i("id")]).unwrap();
    // what 0.32 left: the model's texts, an official translation, a correction by hand (all asked of the
    // official sources)
    store::put(&st.db, "ハルジオン", "en", "title", Some("Daisy Fleabane"), Some("Harujion (model)"), Source::Model).unwrap();
    store::put(&st.db, "夜に駆ける", "en", "title", Some("Racing Into the Night"), None, Source::Official).unwrap();
    store::put(&st.db, "夜に駆ける", "en", "title", Some("Run Into the Night"), Some("Yoru ni Kakeru"), Source::Model).unwrap();
    store::put(&st.db, "ヨルシカ", "en", "artist", None, Some("Yoru Shika"), Source::Model).unwrap();
    store::set_manual(&st.db, "봄날", "en", "title", Some("Spring Day"), None).unwrap();
    for t in ["ハルジオン", "夜に駆ける", "ヨルシカ", "봄날"] {
        store::mark(&st.db, t, "en", "title", Source::Official).unwrap();
    }
    // a new title on screen
    st.library.translator.enqueue("en", vec![("アイドル".into(), Kind::Title, Some("YOASOBI".into()))]);
    pass_now(&st).await;

    let s = shown(&st, "ハルジオン");
    assert_eq!(s.romanization.as_deref(), Some("Harujion"), "the built-in romanization beats the model's");
    assert_eq!((s.translation.as_deref(), s.source.as_str()), (Some("EN:ハルジオン"), "deepl"), "DeepL beats the model");
    let s = shown(&st, "夜に駆ける");
    assert_eq!(s.translation.as_deref(), Some("Racing Into the Night"), "the official name stays");
    assert_eq!(s.romanization.as_deref(), Some("Yoru ni Kakeru"), "kanji: the built-in romanization has none, the model's stays");
    assert_eq!(shown(&st, "ヨルシカ").romanization.as_deref(), Some("Yorushika"));
    let s = shown(&st, "봄날");
    assert_eq!((s.translation.as_deref(), s.romanization.as_deref(), s.source.as_str()), (Some("Spring Day"), Some("Bomnal"), "manual"));
    let s = shown(&st, "アイドル");
    assert_eq!((s.translation.as_deref(), s.romanization.as_deref()), (Some("EN:アイドル"), Some("Aidoru")));
    let mut asked = sent_texts(&seen);
    asked.sort();
    assert_eq!(asked, vec!["アイドル".to_string(), "ハルジオン".to_string()], "DeepL only for titles without a better translation");
    let row = st.db.one("SELECT romanization_source, translation_source FROM translations WHERE text='アイドル'", p![]).unwrap().unwrap();
    assert_eq!((row.s("romanization_source"), row.s("translation_source")), ("romanized".to_string(), "deepl".to_string()));
    // nothing is left unseen, and a second pass asks nobody again
    for source in [Source::Official, Source::Romanized, Source::Deepl] {
        assert!(store::unseen(&st.db, "en", source, 10).is_empty(), "{source:?}");
    }
    pass_now(&st).await;
    assert_eq!(sent_texts(&seen).len(), 2);

    // sources switched off are left out for a new title …
    st.cfg.set_many(&json!({ "translate_romanize": false, "translate_deepl": false, "translate_official": false }));
    st.library.translator.enqueue("en", vec![("ドライフラワー".into(), Kind::Title, None)]);
    pass_now(&st).await;
    let row = st.db.one("SELECT * FROM translations WHERE text='ドライフラワー'", p![]);
    assert!(row.unwrap().is_none(), "no source on: no row");
    st.library.translator.enqueue("en", vec![("ドライフラワー".into(), Kind::Title, None)]);
    st.cfg.set_many(&json!({ "translate_romanize": true }));
    pass_now(&st).await;
    let row = st.db.one("SELECT official_at, deepl_at, romanization FROM translations WHERE text='ドライフラワー'", p![]).unwrap().unwrap();
    assert_eq!(row.s("romanization"), "Doraifurawā");
    assert!(row.get("official_at").is_some_and(|v| v.is_null()) && row.get("deepl_at").is_some_and(|v| v.is_null()));
    // … and reach it once they are on again
    st.cfg.set_many(&json!({ "translate_deepl": true, "translate_official": true }));
    pass_now(&st).await;
    let row = st.db.one("SELECT official_at, translation, translation_source FROM translations WHERE text='ドライフラワー'", p![]).unwrap().unwrap();
    assert!(row.f("official_at") > 0.0);
    assert_eq!((row.s("translation"), row.s("translation_source")), ("EN:ドライフラワー".to_string(), "deepl".to_string()));
    ts.remove();
}

/// Settings → Language: one line for everyone on what this server uses, and for admins every source with its
/// switch and state, strongest first; the local model is not installed by default.
#[tokio::test]
async fn the_language_page_shows_the_sources() {
    let ts = server("tiers-status", "", json!({}));
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    user(&st, "ann", "user");
    let app = ts.app();
    let login = |name: &'static str| {
        let app = app.clone();
        async move {
            let (code, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
            assert_eq!(code, StatusCode::OK);
            cookie.split(';').next().unwrap_or("").to_string()
        }
    };
    let (boss, ann) = (login("boss").await, login("ann").await);
    let (code, s, _) = call(&app, "GET", "/api/translate/status", &boss, None).await;
    assert_eq!(code, StatusCode::OK);
    let tiers = s["tiers"].as_array().unwrap();
    assert_eq!(tiers.iter().map(|t| t["id"].as_str().unwrap()).collect::<Vec<_>>(), ["manual", "official", "romanize", "deepl", "model"]);
    let states: Vec<&str> = tiers.iter().map(|t| t["state"].as_str().unwrap()).collect();
    assert_eq!(states, [
        "Corrections by hand: none yet",
        "Official names: MusicBrainz and Apple",
        "Romanization: built in",
        "DeepL: no API key – add one under Settings → Library → Translated titles",
        "Local model: not installed – install it in Plugins",
    ]);
    assert_eq!(tiers[2]["setting"], "translate_romanize");
    assert_eq!(tiers[0]["setting"], Value::Null);
    assert_eq!(s["translator"], false);
    assert_eq!(s["summary"], "This server uses official names and built-in romanization besides corrections by hand.");
    let (_, s, _) = call(&app, "GET", "/api/translate/status", &ann, None).await;
    assert!(s.get("tiers").is_none() && s.get("rows").is_none(), "admins only");
    assert_eq!(s["summary"], "This server uses official names and built-in romanization besides corrections by hand.");
    // the switches are settings
    let (code, _, _) = call(&app, "PUT", "/api/admin/settings", &boss, Some(json!({ "translate_romanize": false, "deepl_api_key": "k:fx" }))).await;
    assert_eq!(code, StatusCode::OK);
    let (_, s, _) = call(&app, "GET", "/api/translate/status", &boss, None).await;
    let tiers = s["tiers"].as_array().unwrap();
    assert_eq!(tiers[2]["state"], "Romanization: off");
    assert_eq!(tiers[2]["on"], false);
    assert_eq!(tiers[3]["state"], "DeepL: 0 of 400\u{a0}000 characters this month");
    assert_eq!(s["summary"], "This server uses official names and DeepL besides corrections by hand.");
    // the key stays secret
    let (_, v, _) = call(&app, "GET", "/api/admin/settings", &boss, None).await;
    assert_ne!(v["deepl_api_key"], "k:fx");
    ts.remove();
}

/// A row no source can stamp (its source language is not the one its text has now: a row from an older
/// spelling of the script rules) is given to each source once per pass, and the pass ends.
#[tokio::test]
async fn a_row_no_source_can_stamp_does_not_hold_the_pass() {
    let (base, _seen) = stand_in().await;
    let ts = server("tiers-odd-row", &base, json!({ "deepl_api_key": "k-1:fx" }));
    let st = ts.st.clone();
    let u = user(&st, "ann", "user");
    st.db.execute("UPDATE users SET prefs=? WHERE id=?", p![r#"{"translate_mode":"under","translate_lang":"en"}"#, u.i("id")]).unwrap();
    st.db.execute("INSERT INTO translations (text, src_lang, target, kind, updated_at) VALUES ('アイウ', 'ko', 'en', 'title', 1)", p![]).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(60), pass_now(&st)).await.expect("the pass ends");
    // the text's own row got what the sources had
    assert_eq!(shown(&st, "アイウ").romanization.as_deref(), Some("Aiu"));
    ts.remove();
}

/// A group listen playing holds the Translator's model: the model's work does not start (its server is not even
/// started), the rows that wait for it stay waiting (never marked as asked), and the first pass after the last group
/// listen stopped starts it.
#[tokio::test]
async fn a_group_listen_holds_the_model_until_it_stops() {
    use std::os::unix::fs::PermissionsExt;
    let ts = TestServer::new("tiers-group").with_plugins(&["translator"]).with(json!({
        "translate_local_model": true, "translate_background": true,
        "translate_official": false, "translate_romanize": false, "translate_deepl": false,
    }));
    let st = ts.st.clone();
    let u = user(&st, "ann", "user");
    st.db.execute("UPDATE users SET prefs=? WHERE id=?", p![r#"{"translate_mode":"under","translate_lang":"en"}"#, u.i("id")]).unwrap();
    // a stand-in llama-server: notes that it was started, then stops (the start fails, so the model is never asked)
    let started = ts.dir.join("llama-started");
    let exe = ts.dir.join("tools/translator/llama/b6800/build/bin/llama-server");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, format!("#!/bin/sh\necho started >> '{}'\n", started.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(ts.dir.join("tools/translator/llama/current"), "b6800\n").unwrap();
    std::fs::create_dir_all(ts.dir.join("tools/translator/models")).unwrap();
    std::fs::write(ts.dir.join("tools/translator/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"), b"GGUF\x03").unwrap();
    assert!(encore_library::translate::llama::ready(&st));
    // the official sources found nothing for this title: it waits for the model
    store::mark(&st.db, "群青", "en", "title", Source::Official).unwrap();
    let waiting = |st: &crate::state::AppState| store::waiting_for_model(&st.db, "en", 10);
    assert_eq!(waiting(&st).len(), 1);

    st.busy.set_group_playing(true);
    // the background pass and a new title on screen: neither starts the model meanwhile
    pass_now(&st).await;
    st.library.translator.enqueue("en", vec![("夜に駆ける".into(), Kind::Title, None)]);
    pass_now(&st).await;
    assert!(!started.exists(), "the model's server was started while a group listen played");
    assert_eq!(waiting(&st), vec![("群青".to_string(), "title".to_string())], "still waiting, not marked as asked");

    // the last group listen stops: the next pass starts the model for what waits
    assert!(st.busy.set_group_playing(false));
    pass_now(&st).await;
    assert!(started.exists(), "the model's work goes on after the group listen");
    ts.remove();
}
