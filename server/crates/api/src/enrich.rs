//! The enrichment pass: what an answer's music gains before it goes out, for the listener who asked. Every
//! handler whose answer lists albums, songs or artists calls [`enrich`] on it (or [`enrich_now`] when it
//! already runs on a blocking thread):
//! - `cover_v` (the cover's version, for cover URLs that can be cached for good) and `cover_color` (its main
//!   colour) on every album and song, and on an artist that shows an album's cover (`cover_album`); see
//!   `encore_media::covers::stamp`,
//! - `artists` on every song: each artist of a collaboration on its own, for links (`Artists::stamp_credits`),
//! - `<field>_t` / `<field>_r` (translation, romanization) on titles, albums and artists for a listener who
//!   has translations on, and the titles with none yet queued for the Translator (`translate::stamp`),
//! - `gain` (the song's loudness for volume normalization, when known), `sample_rate` and `bit_depth` on
//!   every library track row (0.31, `encore_library::loudness::stamp`; with the covers and credits).
//!
//! Until 0.29 two middlewares did this to every JSON answer of a GET, reading the body back in to do it. The
//! pass does the same, in the same order (covers and credits, then translations, which see the credits) and
//! on the same condition: covers and credits only when the answer names an album or a cover
//! ([`mentions_albums`]). enrich_tests.rs holds the answers to the ones 0.28 gave
//! (`answers_match_the_captured_fixtures`) and asks every GET that can carry music whether its answer went
//! through the pass (`every_answer_is_already_enriched`): a new handler that lists music calls it too, and
//! joins that list.
use crate::auth;
use encore_db::Row;
use crate::state::AppState;
use encore_library::translate::{self, Mode};
use axum::http::HeaderMap;
use serde_json::Value;

/// Adds covers, credits and translations to `v` for `user` (None: nobody signed in, or a handler that
/// has no user; see [`viewer`]). The work runs on the blocking pool (it reads the database and the cover
/// files); nothing happens, and nothing is moved, when there is nothing to add.
pub async fn enrich(st: &AppState, user: Option<&Row>, v: &mut Value) {
    let covers = mentions_albums(v);
    let target = target_of(user);
    if !covers && target.is_none() {
        return;
    }
    let st2 = st.clone();
    let mut taken = std::mem::take(v);
    *v = tokio::task::spawn_blocking(move || {
        apply(&st2, covers, target, &mut taken);
        taken
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("enrich: {e}");
        Value::Null
    });
}

/// [`enrich`] on the calling thread, for code that already runs on a blocking thread.
pub fn enrich_now(st: &AppState, user: Option<&Row>, v: &mut Value) {
    let covers = mentions_albums(v);
    let target = target_of(user);
    if covers || target.is_some() {
        apply(st, covers, target, v);
    }
}

/// The signed-in listener of a request whose handler has no user extractor (a share link's data, which
/// anyone may open): looked up only while somebody has translations on, since only translations differ
/// per listener.
pub async fn viewer(st: &AppState, headers: &HeaderMap) -> Option<Row> {
    let (st2, headers) = (st.clone(), headers.clone());
    tokio::task::spawn_blocking(move || {
        if !st2.library.translator.anyone_on(&st2.db) {
            return None;
        }
        auth::current_user(&st2, &headers)
    })
    .await
    .ok()
    .flatten()
}

/// The language a listener reads translations in, when they have them on.
fn target_of(user: Option<&Row>) -> Option<&'static str> {
    let (mode, target) = translate::prefs_of(user?);
    (mode != Mode::Off).then_some(target)
}

fn apply(st: &AppState, covers: bool, target: Option<&'static str>, v: &mut Value) {
    // a panic in a stamp leaves the answer as far as it got, as the middlewares sent the body unchanged
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if covers {
            encore_media::covers::stamp(st, v);
            st.library.artists.stamp_credits(&st.db, v);
            // every track row names its album, so this is the place for its loudness and format details
            encore_library::loudness::stamp(&st.db, v);
        }
        // while nobody has translations on this is one look at a cached list
        if let Some(t) = target.filter(|_| st.library.translator.anyone_on(&st.db)) {
            let missing = translate::stamp(&st.db, t, v);
            st.library.translator.enqueue(t, missing);
        }
    }));
    if done.is_err() {
        tracing::warn!("enrich: a stamp failed; the answer goes out as far as it got");
    }
}

/// Does the answer name an album or a cover? The middleware looked for the bytes `"album"`, `"album_` or
/// `"cover_` in the JSON text; this is the same test on the value: a key or a string that is `album`, starts
/// with `album_` or `cover_`, or holds a quote followed by those (a quote inside a string is written `\"`,
/// so its quote starts the same bytes).
pub fn mentions_albums(v: &Value) -> bool {
    fn names(s: &str) -> bool {
        s == "album" || s.ends_with("\"album") || s.starts_with("album_") || s.starts_with("cover_") || s.contains("\"album_") || s.contains("\"cover_")
    }
    match v {
        Value::String(s) => names(s),
        Value::Array(a) => a.iter().any(mentions_albums),
        Value::Object(o) => o.iter().any(|(k, x)| names(k) || mentions_albums(x)),
        _ => false,
    }
}

#[cfg(test)]
#[path = "enrich_tests.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The value test says yes exactly where the middleware's byte test did.
    #[test]
    fn mentions_albums_matches_the_byte_test() {
        let bytes_say = |v: &Value| {
            let b = serde_json::to_vec(v).unwrap();
            b.windows(7).any(|w| w == b"\"album\"" || w == b"\"album_" || w == b"\"cover_")
        };
        let cases = [
            json!({ "album": "x" }),
            json!({ "albums": 3 }),
            json!({ "album_id": "ab" }),
            json!([{ "cover_album": "ab" }]),
            json!({ "kind": "album" }),
            json!({ "kind": "albums" }),
            json!({ "title": "My album" }),
            json!({ "title": "album_art.jpg" }),
            json!({ "title": "cover_story" }),
            json!({ "title": "the \"album\" said" }),
            json!({ "title": "say \"album" }),
            json!({ "title": "a \"cover_ thing" }),
            json!({ "title": "x\\\"album_" }),
            json!({ "albumartist": "A" }),
            json!({ "covers": ["a"] }),
            json!({ "deep": [[{ "x": [{ "album_t": 1 }] }]] }),
            json!(["album"]),
            json!("cover_v"),
            json!(42),
            json!(null),
            json!({}),
        ];
        for v in cases {
            assert_eq!(mentions_albums(&v), bytes_say(&v), "{v}");
        }
    }

    /// Translations go to a listener who has them on, after the credits (so an artist of a collaboration
    /// shows its own translation through `artists`), and titles with no translation yet are queued.
    #[tokio::test]
    async fn translations_follow_the_listener() {
        let s = crate::test_support::TestServer::new("enrich-translate");
        let st = s.st.clone();
        st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id) VALUES (1, '/m/1.flac', '真夏の果実', 'サザン', 'アルバム', 'サザン', 'aa00aa00aa00aa00')", encore_db::p![]).unwrap();
        encore_library::translate::store::set_manual(&st.db, "真夏の果実", "en", "title", Some("Midsummer Fruit"), Some("Manatsu no Kajitsu")).unwrap();
        st.db.execute("UPDATE users SET prefs=? WHERE id=1", encore_db::p![r#"{"translate_mode":"under","translate_lang":"en"}"#]).unwrap();
        let reader = st.db.one("SELECT * FROM users WHERE id=1", encore_db::p![]).unwrap().unwrap();
        let song = || json!([{ "id": 1, "title": "真夏の果実", "artist": "サザン", "album": "アルバム", "album_id": "aa00aa00aa00aa00" }]);
        // nobody signed in, and somebody who has translations off: covers and credits only
        let mut off = reader.clone();
        off.insert("prefs".into(), json!("{}"));
        for u in [None, Some(&off)] {
            let mut v = song();
            enrich(&st, u, &mut v).await;
            assert!(v[0].get("title_t").is_none(), "{v}");
            assert_eq!(v[0]["artists"][0]["name"], "サザン", "{v}");
        }
        let mut v = song();
        enrich(&st, Some(&reader), &mut v).await;
        assert_eq!(v[0]["title_t"], "Midsummer Fruit", "{v}");
        assert_eq!(v[0]["title_r"], "Manatsu no Kajitsu", "{v}");
        // the rest has no translation yet: queued for the Translator
        assert!(st.library.translator.queued() >= 1);
        // the same on the calling thread
        let mut w = song();
        tokio::task::spawn_blocking({
            let st = st.clone();
            move || {
                enrich_now(&st, Some(&reader), &mut w);
                w
            }
        }).await.map(|w| assert_eq!(w, v)).unwrap();
        // no album, no cover: nothing is stamped but the translation
        let mut plain = json!({ "title": "真夏の果実" });
        enrich(&st, None, &mut plain).await;
        assert_eq!(plain, json!({ "title": "真夏の果実" }));
        s.remove();
    }
}
