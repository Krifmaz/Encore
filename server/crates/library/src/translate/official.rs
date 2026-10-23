//! Official translations: what the labels and the MusicBrainz community published, preferred over
//! anything the model makes up.
//!
//! * MusicBrainz pseudo-releases: a release linked by "transl-tracklisting" to a pseudo-release whose
//!   text is in the listener's language (a translation) or in Latin script (a transliteration, which
//!   becomes the romanization), matched track by track.
//! * MusicBrainz aliases: release aliases and artist aliases with the listener's locale (and
//!   `<lang>-Latn` aliases as romanizations).
//! * Apple Music: the English names Apple shows for Japanese releases (the iTunes Search API answers in
//!   English or Japanese only, so this is used for Japanese titles read in English).
//! * Deezer has no localized titles in its API, so it is not asked.
use encore_core::json::s;
use super::prompt::Kind;
use crate::Ctx;
use super::script;
use serde_json::Value;
use std::collections::HashMap;

/// One official result: (kind, original text, translation, romanization).
pub type Found = (Kind, String, Option<String>, Option<String>);

/// What an official pseudo-release gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// a translation of the titles
    Translation,
    /// a transliteration (romanization) of the titles
    Transliteration,
}

/// The pseudo-releases of a release (MusicBrainz release JSON with `release-rels`) that help a
/// listener of `target`: (release id, translation or transliteration).
pub fn pseudo_links(release: &Value, target: &str) -> Vec<(String, Role)> {
    let want = script::iso639_3(target);
    // a transliteration keeps the release's own language, only the script changes
    let own = s(&release["text-representation"]["language"]);
    let mut out = vec![];
    for r in release["relations"].as_array().into_iter().flatten() {
        if r["type"].as_str() != Some("transl-tracklisting") {
            continue;
        }
        let rel = &r["release"];
        let id = s(&rel["id"]);
        if id.is_empty() || id == s(&release["id"]) {
            continue;
        }
        let tr = &rel["text-representation"];
        let (lang, scr) = (s(&tr["language"]), s(&tr["script"]));
        let role = if !want.is_empty() && lang == want {
            Role::Translation
        } else if scr == "Latn" && (lang.is_empty() || lang == own || lang == "mul" || lang == "zxx") {
            Role::Transliteration
        } else {
            continue;
        };
        if !out.iter().any(|(i, r): &(String, Role)| *i == id && *r == role) {
            out.push((id, role));
        }
    }
    // translations first: they also give a title when there is no transliteration
    out.sort_by_key(|(_, r)| *r != Role::Translation);
    out
}

/// (disc, track) → title of a MusicBrainz release with `recordings`.
fn titles(release: &Value) -> HashMap<(i64, i64), String> {
    let mut out = HashMap::new();
    for m in release["media"].as_array().into_iter().flatten() {
        let disc = m["position"].as_i64().unwrap_or(1);
        for t in m["tracks"].as_array().into_iter().flatten() {
            let pos = t["position"].as_i64().or_else(|| s(&t["number"]).parse().ok()).unwrap_or(0);
            let title = { let x = s(&t["title"]); if x.is_empty() { s(&t["recording"]["title"]) } else { x } };
            if !title.is_empty() {
                out.insert((disc, pos), title);
            }
        }
    }
    out
}

/// Original title → pseudo-release title, track by track (same disc and position), plus the
/// release titles. Titles that did not change are left out.
pub fn pair_titles(original: &Value, pseudo: &Value) -> Vec<(Kind, String, String)> {
    let mut out = vec![];
    let (a, b) = (s(&original["title"]), s(&pseudo["title"]));
    if !a.is_empty() && !b.is_empty() && !script::same(&a, &b) {
        out.push((Kind::Album, a, b));
    }
    let theirs = titles(pseudo);
    let mut mine: Vec<((i64, i64), String)> = titles(original).into_iter().collect();
    mine.sort();
    for (k, t) in mine {
        if let Some(p) = theirs.get(&k) {
            if !script::same(&t, p) && !out.iter().any(|(_, o, _)| *o == t) {
                out.push((Kind::Title, t, p.clone()));
            }
        }
    }
    out
}

/// From MusicBrainz aliases (artist or release `aliases`): (translation, romanization) for `target`.
/// A primary alias in the target locale wins over others; `<src>-Latn` / `<src>_Latn` locales are
/// romanizations; for a Latin-script target an alias in the target locale that is in Latin letters is
/// both. Search hints are skipped.
pub fn pick_aliases(aliases: &Value, target: &str, original: &str) -> (Option<String>, Option<String>) {
    let list: Vec<&Value> = aliases.as_array().into_iter().flatten().filter(|a| a["type"].as_str() != Some("Search hint")).collect();
    let locale = |a: &Value| a["locale"].as_str().unwrap_or("").to_ascii_lowercase().replace('_', "-");
    let mut tr: Option<(bool, String)> = None;
    let mut ro: Option<String> = None;
    for a in &list {
        let (loc, name) = (locale(a), s(&a["name"]));
        if name.is_empty() || script::same(&name, original) {
            continue;
        }
        let primary = a["primary"].as_bool().unwrap_or(false);
        if loc.ends_with("-latn") {
            ro.get_or_insert(name);
        } else if (loc == target || loc.starts_with(&format!("{target}-"))) && tr.as_ref().is_none_or(|(p, _)| !p && primary) {
            tr = Some((primary, name));
        }
    }
    let tr = tr.map(|(_, n)| n);
    // an English alias of a Japanese name is in Latin letters: it is how the name is written in English
    if ro.is_none() && script::detect(original) != "latn" {
        if let Some(t) = tr.as_ref().filter(|t| script::detect(t) == "latn" && script::target_script(target) == "latn") {
            ro = Some(t.clone());
        }
    }
    (tr, ro)
}

/// Apple: (disc, track) → name from an iTunes lookup of an album with `entity=song`, and the album name.
pub fn apple_titles(lookup: &Value) -> (Option<String>, HashMap<(i64, i64), String>) {
    let mut album = None;
    let mut out = HashMap::new();
    for r in lookup["results"].as_array().into_iter().flatten() {
        match r["wrapperType"].as_str() {
            Some("collection") => album = r["collectionName"].as_str().map(String::from),
            Some("track") => {
                if let (Some(d), Some(t), Some(n)) = (r["discNumber"].as_i64(), r["trackNumber"].as_i64(), r["trackName"].as_str()) {
                    out.insert((d, t), n.to_string());
                }
            }
            _ => {}
        }
    }
    (album, out)
}

/// Pair two iTunes lookups of the same album (in Japanese and in English).
pub fn apple_pairs(native: &Value, english: &Value) -> Vec<(Kind, String, String)> {
    let (na, nt) = apple_titles(native);
    let (ea, et) = apple_titles(english);
    let mut out = vec![];
    if let (Some(a), Some(b)) = (na, ea) {
        if !script::same(&a, &b) {
            out.push((Kind::Album, a, b));
        }
    }
    let mut keys: Vec<_> = nt.keys().copied().collect();
    keys.sort();
    for k in keys {
        if let (Some(a), Some(b)) = (nt.get(&k), et.get(&k)) {
            if !script::same(a, b) {
                out.push((Kind::Title, a.clone(), b.clone()));
            }
        }
    }
    out
}

// ---- lookups -----------------------------------------------------------------------------------

/// Official translations of a MusicBrainz release's title and track titles.
pub async fn release(st: &impl Ctx, rid: &str, target: &str) -> anyhow::Result<Vec<Found>> {
    let orig = st.mb().get(st, &format!("release/{rid}"), &[("inc", "release-rels+recordings+aliases".into())]).await?;
    let mut out: Vec<Found> = vec![];
    let title = s(&orig["title"]);
    let (t, r) = pick_aliases(&orig["aliases"], target, &title);
    if t.is_some() || r.is_some() {
        out.push((Kind::Album, title.clone(), t, r));
    }
    for (pid, role) in pseudo_links(&orig, target).into_iter().take(3) {
        let pseudo = st.mb().get(st, &format!("release/{pid}"), &[("inc", "recordings".into())]).await?;
        for (kind, from, to) in pair_titles(&orig, &pseudo) {
            let (tr, ro) = match role {
                Role::Translation => (Some(to), None),
                Role::Transliteration => (None, Some(to)),
            };
            if let Some(e) = out.iter_mut().find(|e| e.1 == from) {
                if e.2.is_none() { e.2 = tr; }
                if e.3.is_none() { e.3 = ro; }
            } else {
                out.push((kind, from, tr, ro));
            }
        }
    }
    Ok(out)
}

/// Official names of a MusicBrainz artist for `target`; without a romanization among the aliases, the artist's
/// sort name gives one when it is in Latin letters (see [`sort_name_reading`]).
pub async fn artist(st: &impl Ctx, mbid: &str, name: &str, target: &str) -> anyhow::Result<(Option<String>, Option<String>)> {
    let a = st.mb().get(st, &format!("artist/{mbid}"), &[("inc", "aliases".into())]).await?;
    let (t, r) = pick_aliases(&a["aliases"], target, name);
    Ok((t, r.or_else(|| sort_name_reading(&s(&a["sort-name"]), name))))
}

/// The reading of a name in another script from MusicBrainz's sort name, when that is in Latin letters:
/// "Yonezu, Kenshi" for 米津玄師 reads "Yonezu Kenshi" (the order the sort name keeps is the name's own),
/// "Beatles, The" reads "The Beatles". None when the name is in Latin letters already or the sort name is not.
pub fn sort_name_reading(sort: &str, name: &str) -> Option<String> {
    let sort = sort.trim();
    if sort.is_empty() || matches!(script::detect(name), "latn" | "") || script::detect(sort) != "latn" || script::same(sort, name) {
        return None;
    }
    let reading = match sort.rsplit_once(", ") {
        Some((head, article)) if ["The", "A", "An"].contains(&article) => format!("{article} {head}"),
        Some((family, given)) if !family.contains(',') => format!("{family} {given}"),
        _ => sort.to_string(),
    };
    Some(reading)
}

// Apple's iTunes Search API, paced (shared with every other caller) and tried once more when throttled
use encore_http::apple;

/// Apple's English titles of a Japanese album (only Japanese → English: the API offers no other pair).
pub async fn apple(st: &impl Ctx, artist: &str, album: &str, target: &str) -> anyhow::Result<Vec<Found>> {
    if target != "en" || script::detect(album) != "ja" && script::detect(artist) != "ja" {
        return Ok(vec![]);
    }
    let found = apple::get(st, apple::search(st, &format!("{artist} {album}"), "album", 10).query(&[("country", "JP"), ("lang", "ja_jp")])).await?;
    let Some(id) = found["results"].as_array().into_iter().flatten()
        .find(|r| script::same(r["collectionName"].as_str().unwrap_or(""), album))
        .and_then(|r| r["collectionId"].as_i64()) else { return Ok(vec![]) };
    let id = id.to_string();
    let native = apple::get(st, apple::lookup(st, &id, "song", None).query(&[("country", "JP"), ("lang", "ja_jp")])).await?;
    let english = apple::get(st, apple::lookup(st, &id, "song", None).query(&[("country", "JP"), ("lang", "en_us")])).await?;
    Ok(apple_pairs(&native, &english).into_iter().map(|(k, a, b)| (k, a, Some(b), None)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn original() -> Value {
        json!({
            "id": "orig", "title": "THE BOOK", "text-representation": { "language": "jpn", "script": "Jpan" },
            "relations": [
                { "type": "transl-tracklisting", "direction": "forward", "release": { "id": "eng", "text-representation": { "language": "eng", "script": "Latn" } } },
                { "type": "transl-tracklisting", "direction": "forward", "release": { "id": "rom", "text-representation": { "language": "jpn", "script": "Latn" } } },
                { "type": "transl-tracklisting", "release": { "id": "kor", "text-representation": { "language": "kor", "script": "Kore" } } },
                { "type": "remaster", "release": { "id": "other" } },
            ],
            "media": [{ "position": 1, "tracks": [
                { "position": 1, "title": "夜に駆ける" }, { "position": 2, "title": "群青" }, { "position": 3, "title": "Epilogue" },
            ] }],
        })
    }

    #[test]
    fn pseudo_releases_by_language_and_script() {
        assert_eq!(pseudo_links(&original(), "en"), vec![("eng".to_string(), Role::Translation), ("rom".to_string(), Role::Transliteration)]);
        // a German reader: no German pseudo-release; the English one is a translation, not a romanization
        assert_eq!(pseudo_links(&original(), "de"), vec![("rom".to_string(), Role::Transliteration)]);
        assert_eq!(pseudo_links(&original(), "ko"), vec![("kor".to_string(), Role::Translation), ("rom".to_string(), Role::Transliteration)]);
        assert!(pseudo_links(&json!({ "id": "x" }), "en").is_empty());
    }

    #[test]
    fn titles_pair_by_position() {
        let pseudo = json!({ "id": "eng", "title": "THE BOOK", "media": [{ "position": 1, "tracks": [
            { "position": 1, "title": "Racing Into The Night" }, { "position": 2, "recording": { "title": "Gunjou" } }, { "position": 3, "title": "Epilogue" },
        ] }] });
        let p = pair_titles(&original(), &pseudo);
        assert_eq!(p, vec![
            (Kind::Title, "夜に駆ける".to_string(), "Racing Into The Night".to_string()),
            (Kind::Title, "群青".to_string(), "Gunjou".to_string()),
        ], "unchanged titles (the album, Epilogue) are left out");
    }

    #[test]
    fn aliases_by_locale() {
        let aliases = json!([
            { "name": "Utada Hikaru", "locale": "en", "primary": false, "type": "Artist name" },
            { "name": "Hikaru Utada", "locale": "en", "primary": true, "type": "Artist name" },
            { "name": "Utada Hikaru", "locale": "ja_Latn", "primary": true, "type": "Artist name" },
            { "name": "utada", "locale": "en", "primary": true, "type": "Search hint" },
        ]);
        assert_eq!(pick_aliases(&aliases, "en", "宇多田ヒカル"), (Some("Hikaru Utada".into()), Some("Utada Hikaru".into())));
        // no -Latn alias: the English (Latin) name romanizes too
        let en_only = json!([{ "name": "Hikaru Utada", "locale": "en_US", "primary": true }]);
        assert_eq!(pick_aliases(&en_only, "en", "宇多田ヒカル"), (Some("Hikaru Utada".into()), Some("Hikaru Utada".into())));
        assert_eq!(pick_aliases(&json!([]), "en", "x"), (None, None));
        assert_eq!(pick_aliases(&json!([{ "name": "Kino", "locale": "de" }]), "en", "Кино"), (None, None));
    }

    #[test]
    fn sort_names_give_readings() {
        assert_eq!(sort_name_reading("Yonezu, Kenshi", "米津玄師").as_deref(), Some("Yonezu Kenshi"));
        assert_eq!(sort_name_reading("Beatles, The", "ザ・ビートルズ").as_deref(), Some("The Beatles"));
        assert_eq!(sort_name_reading("Yorushika", "ヨルシカ").as_deref(), Some("Yorushika"));
        assert_eq!(sort_name_reading("米津玄師", "米津玄師"), None, "not in Latin letters");
        assert_eq!(sort_name_reading("Beatles, The", "The Beatles"), None, "the name is in Latin letters");
        assert_eq!(sort_name_reading("", "ヨルシカ"), None);
    }

    #[test]
    fn apple_lookups_pair_by_disc_and_track() {
        let native = json!({ "results": [
            { "wrapperType": "collection", "collectionName": "THE BOOK" },
            { "wrapperType": "track", "discNumber": 1, "trackNumber": 1, "trackName": "夜に駆ける" },
            { "wrapperType": "track", "discNumber": 1, "trackNumber": 2, "trackName": "ハルジオン" },
        ] });
        let english = json!({ "results": [
            { "wrapperType": "collection", "collectionName": "THE BOOK" },
            { "wrapperType": "track", "discNumber": 1, "trackNumber": 1, "trackName": "Racing Into the Night" },
            { "wrapperType": "track", "discNumber": 1, "trackNumber": 2, "trackName": "ハルジオン" },
        ] });
        assert_eq!(apple_pairs(&native, &english), vec![(Kind::Title, "夜に駆ける".to_string(), "Racing Into the Night".to_string())]);
    }
}
