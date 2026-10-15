//! Cache keys, official-first precedence, manual corrections and stamping (no model needed).
use super::*;
use crate::db::Db;
use crate::p;
use store::Source;

fn db(name: &str) -> (Db, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("encore-translate-{name}-{}-{}", std::process::id(), crate::auth::random_hex(4)));
    let _ = std::fs::remove_dir_all(&d);
    (Db::open(&d).unwrap(), d)
}

fn shown(db: &Db, text: &str, target: &str) -> Option<store::Shown> {
    store::lookup(db, target, &[text.to_string()]).remove(&script::normalize(text))
}

#[test]
fn cache_keys_follow_the_text() {
    assert_eq!(store::key("  夜に駆ける "), ("夜に駆ける".to_string(), "ja"));
    assert_eq!(store::key("Кино"), ("Кино".to_string(), "cyrl"));
    let (db, dir) = db("keys");
    store::put(&db, "夜に駆ける", "en", "title", Some("Racing Into the Night"), Some("Yoru ni Kakeru"), Source::Model).unwrap();
    // the same text with other spacing is the same row; another target is another row
    store::put(&db, " 夜に駆ける", "en", "title", Some("Run Into the Night"), None, Source::Model).unwrap();
    store::put(&db, "夜に駆ける", "de", "title", Some("In die Nacht rennen"), None, Source::Model).unwrap();
    let n = db.one("SELECT COUNT(*) n FROM translations", p![]).unwrap().unwrap();
    assert_eq!(n.i("n"), 2);
    let r = db.one("SELECT src_lang FROM translations WHERE target='en'", p![]).unwrap().unwrap();
    assert_eq!(r.s("src_lang"), "ja");
    assert_eq!(shown(&db, "夜に駆ける", "en").unwrap().translation.as_deref(), Some("Run Into the Night"), "the same source replaces itself");
    assert_eq!(shown(&db, "夜に駆ける", "de").unwrap().translation.as_deref(), Some("In die Nacht rennen"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn official_beats_the_model_field_by_field() {
    let (db, dir) = db("precedence");
    // the model came first
    store::put(&db, "夜に駆ける", "en", "title", Some("Run Into the Night"), Some("Yoru ni Kakeru"), Source::Model).unwrap();
    // an official translation replaces it; it has no romanization, so the model's stays
    store::put(&db, "夜に駆ける", "en", "title", Some("Racing Into the Night"), None, Source::Official).unwrap();
    let s = shown(&db, "夜に駆ける", "en").unwrap();
    assert_eq!((s.translation.as_deref(), s.romanization.as_deref(), s.source.as_str()), (Some("Racing Into the Night"), Some("Yoru ni Kakeru"), "official"));
    // the model again (a later pass) does not overwrite the official translation, but may not
    // touch the romanization it gave either way
    store::put(&db, "夜に駆ける", "en", "title", Some("Night Run"), Some("Yoru ni kakeru"), Source::Model).unwrap();
    let s = shown(&db, "夜に駆ける", "en").unwrap();
    assert_eq!(s.translation.as_deref(), Some("Racing Into the Night"));
    assert_eq!(s.romanization.as_deref(), Some("Yoru ni kakeru"));
    // a translation that only repeats the original is never stored
    store::put(&db, "群青", "en", "title", Some("群青"), None, Source::Official).unwrap();
    assert!(shown(&db, "群青", "en").is_none());
    // marking: asked, nothing found
    store::mark(&db, "ハルジオン", "en", "title", Source::Official).unwrap();
    let r = db.one("SELECT official_at, model_at FROM translations WHERE text='ハルジオン'", p![]).unwrap().unwrap();
    assert!(r.f("official_at") > 0.0 && r.get("model_at").is_some_and(|v| v.is_null()));
    // both were asked of the official sources and found nothing: the model gets them next
    let mut waiting = store::waiting_for_model(&db, "en", 10);
    waiting.sort();
    assert_eq!(waiting, vec![("ハルジオン".to_string(), "title".to_string()), ("群青".to_string(), "title".to_string())]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn manual_corrections_win_and_stick() {
    let (db, dir) = db("manual");
    store::put(&db, "Кино", "en", "artist", Some("Cinema"), Some("Kino"), Source::Model).unwrap();
    store::set_manual(&db, "Кино", "en", "artist", Some(""), Some("Kino")).unwrap();
    let s = shown(&db, "Кино", "en").unwrap();
    assert_eq!((s.translation, s.romanization.as_deref(), s.source.as_str()), (None, Some("Kino"), "manual"), "empty = show no translation");
    // later sources do not undo it
    store::put(&db, "Кино", "en", "artist", Some("Movie"), Some("Kino"), Source::Official).unwrap();
    let s = shown(&db, "Кино", "en").unwrap();
    assert_eq!((s.translation, s.source.as_str()), (None, "manual"));
    let e = store::entry(&db, "Кино", "en");
    assert_eq!(e["manual"], true);
    assert_eq!(e["auto_translation"], "Movie");
    assert!(store::waiting_for_model(&db, "en", 10).is_empty(), "a corrected row never goes to the model");
    // removing the correction shows what the sources found
    store::set_manual(&db, "Кино", "en", "artist", None, None).unwrap();
    let s = shown(&db, "Кино", "en").unwrap();
    assert_eq!((s.translation.as_deref(), s.source.as_str()), (Some("Movie"), "official"));
    assert_eq!(store::entry(&db, "Кино", "en")["manual"], false);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_answers_gain_t_and_r_fields() {
    let (db, dir) = db("stamp");
    store::put(&db, "夜に駆ける", "en", "title", Some("Racing Into the Night"), Some("Yoru ni Kakeru"), Source::Official).unwrap();
    store::put(&db, "YOASOBI", "en", "artist", Some("x"), None, Source::Official).unwrap();
    store::put(&db, "ヨルシカ", "en", "artist", None, Some("Yorushika"), Source::Official).unwrap();
    let mut v = json!({
        "tracks": [
            { "id": 1, "title": "夜に駆ける", "artist": "YOASOBI", "album": "THE BOOK" },
            { "id": 2, "title": "ただ君に晴れ", "artist": "ヨルシカ", "album": "負け犬にアンコールはいらない" },
        ],
        "artists": [{ "name": "ヨルシカ", "albums": 3, "cover_album": "a1" }],
        "user": { "username": "ヨルシカ", "name": "ヨルシカ" },
    });
    let missing = stamp(&db, "en", &mut v);
    assert_eq!(v["tracks"][0]["title_t"], "Racing Into the Night");
    assert_eq!(v["tracks"][0]["title_r"], "Yoru ni Kakeru");
    assert_eq!(v["tracks"][0]["artist_t"], "x", "any stored text is shown, Latin or not");
    assert!(v["tracks"][0].get("album_t").is_none());
    assert_eq!(v["tracks"][1]["artist_r"], "Yorushika");
    assert!(v["tracks"][1].get("artist_t").is_none());
    assert_eq!(v["artists"][0]["name_r"], "Yorushika");
    assert!(v["user"].get("name_r").is_none(), "users are not artists");
    // what is worth translating and has no row yet is returned to be queued
    let mut m: Vec<String> = missing.iter().map(|(t, _, _)| t.clone()).collect();
    m.sort();
    assert_eq!(m, vec!["ただ君に晴れ".to_string(), "負け犬にアンコールはいらない".to_string()]);
    let (_, kind, artist) = missing.iter().find(|(t, _, _)| t == "ただ君に晴れ").unwrap();
    assert_eq!((*kind, artist.as_deref()), (Kind::Title, Some("ヨルシカ")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn prefs_and_queue() {
    let row = |p: &str| -> Row { serde_json::from_value(json!({ "prefs": p })).unwrap() };
    assert_eq!(prefs_of(&row(r#"{"translate_mode":"under","translate_lang":"de"}"#)), (Mode::Under, "de"));
    assert_eq!(prefs_of(&row(r#"{"translate_mode":"romanized"}"#)), (Mode::Romanized, "en"));
    assert_eq!(prefs_of(&row("{}")), (Mode::Off, "en"));
    assert_eq!(prefs_of(&row("not json")), (Mode::Off, "en"));
    // the same text for the same target is queued once
    let text = format!("テスト{}", crate::auth::random_hex(4));
    enqueue("ko", vec![(text.clone(), Kind::Title, None), (format!(" {text} "), Kind::Title, None)]);
    let q = QUEUE.lock().unwrap();
    assert_eq!(q.0.iter().filter(|p| p.text == text && p.target == "ko").count(), 1);
}

#[test]
fn active_targets_come_from_users_with_translations_on() {
    let (db, dir) = db("targets");
    for (i, prefs) in [r#"{"translate_mode":"instead","translate_lang":"ja"}"#, r#"{"translate_mode":"off","translate_lang":"de"}"#, r#"{"translate_mode":"under"}"#, "{}"].iter().enumerate() {
        db.execute("INSERT INTO users (username, password_hash, role, created_at, prefs) VALUES (?,?,?,?,?)", p![format!("u{i}"), "x", "user", 0.0, prefs]).unwrap();
    }
    assert_eq!(active_targets(&db), vec!["en", "ja"]);
    let _ = std::fs::remove_dir_all(dir);
}
