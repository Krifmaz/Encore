//! The prompt the local model gets and how its answer is read. No model is needed to test this.
use super::script;
use serde_json::{json, Value};
use std::collections::HashMap;

/// What one item is, for the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// a song title
    Title,
    /// an album title
    Album,
    /// an artist name
    Artist,
}

impl Kind {
    /// Its name in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Title => "title",
            Kind::Album => "album",
            Kind::Artist => "artist",
        }
    }
    /// The kind a stored name stands for (song title for anything unknown).
    pub fn parse(s: &str) -> Kind {
        match s {
            "album" => Kind::Album,
            "artist" | "albumartist" | "name" => Kind::Artist,
            _ => Kind::Title,
        }
    }
    /// What the prompt calls it.
    fn describe(self) -> &'static str {
        match self {
            Kind::Title => "song title",
            Kind::Album => "album title",
            Kind::Artist => "artist name",
        }
    }
}

/// One text to translate, with the artist as context.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// its number in the request
    pub id: usize,
    /// what it is
    pub kind: Kind,
    /// the text to translate
    pub text: String,
    /// the artist, as context
    pub artist: Option<String>,
}

/// Items per request: small enough that a 4B model keeps the ids straight.
pub const BATCH: usize = 16;

/// The system prompt for translating into `target`.
pub fn system_prompt(target: &str) -> String {
    let lang = script::language_name(target);
    format!(
        "You translate music metadata (song titles, album titles, artist names) into {lang}.\n\
Rules:\n\
- Keep proper nouns (people, places, brands, band names) as they are, or in their usual {lang} spelling.\n\
- Keep stylization (capital letters, symbols, punctuation, brackets), numbers, and any part already written in Latin letters exactly as written.\n\
- \"translation\": the meaning in {lang}; use the official {lang} title when one is well known. Use null when the text is already {lang} or is only a name.\n\
- \"romanization\": for text in a non-Latin script, its standard romanization (Hepburn for Japanese, Hanyu Pinyin without tone marks for Chinese, Revised Romanization for Korean, BGN/PCGN for Cyrillic and Greek). Use null for text already in Latin letters.\n\
- Artist names: give the romanization, and a translation only when the artist is known by a {lang} name.\n\
- Never explain, never add quotes or notes.\n\
Answer with JSON only, in this shape: {{\"items\":[{{\"id\":1,\"translation\":\"…\",\"romanization\":\"…\"}}]}} with one entry per id."
    )
}

/// The request's text: the items as JSON (and `/no_think` for a thinking model).
pub fn user_prompt(items: &[Item], thinking_model: bool) -> String {
    let list: Vec<Value> = items.iter().map(|i| {
        let mut v = json!({ "id": i.id, "kind": i.kind.describe(), "text": i.text });
        if let Some(a) = i.artist.as_deref().filter(|a| !a.trim().is_empty() && *a != i.text) {
            v["artist"] = json!(a);
        }
        v
    }).collect();
    // Qwen3's hybrid models think before answering unless told not to
    format!("{}{}", json!({ "items": list }), if thinking_model { "\n/no_think" } else { "" })
}

/// The chat messages for one batch.
pub fn messages(items: &[Item], target: &str, thinking_model: bool) -> Value {
    json!([
        { "role": "system", "content": system_prompt(target) },
        { "role": "user", "content": user_prompt(items, thinking_model) },
    ])
}

/// The model's answer, with any <think> block, code fence or chatter around the JSON removed.
fn json_part(reply: &str) -> Option<Value> {
    let mut text = reply.to_string();
    while let Some(start) = text.find("<think>") {
        match text[start..].find("</think>") {
            Some(end) => text.replace_range(start..start + end + "</think>".len(), ""),
            None => text.truncate(start),
        }
    }
    let text = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    // the outermost {...} or [...]
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(a), Some(b)) = (text.find(open), text.rfind(close)) {
            if a < b {
                if let Ok(v) = serde_json::from_str::<Value>(&text[a..=b]) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// A model's answer cleaned: quotes trimmed; None when empty, a "null"/"none"/"n/a", too long, or the original again.
fn clean(v: &Value, original: &str) -> Option<String> {
    let s = v.as_str()?.trim().trim_matches(['"', '\u{201C}', '\u{201D}']).trim();
    if s.is_empty() || s.eq_ignore_ascii_case("null") || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("n/a") || s.len() > 500 {
        return None;
    }
    (!script::same(s, original)).then(|| s.to_string())
}

/// Read the answer for `items`: id → (translation, romanization). Items the answer leaves out are
/// missing from the map; a value that only repeats the original becomes None. A romanization is only
/// kept when it is in Latin letters.
pub fn parse(reply: &str, items: &[Item]) -> HashMap<usize, (Option<String>, Option<String>)> {
    let mut out = HashMap::new();
    let Some(v) = json_part(reply) else { return out };
    let list = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("items").or_else(|| o.get("results")).and_then(|x| x.as_array()).cloned()
            .unwrap_or_else(|| if o.contains_key("id") { vec![v.clone()] } else { vec![] }),
        _ => vec![],
    };
    for (pos, e) in list.iter().enumerate() {
        let id = match &e["id"] {
            Value::Number(n) => n.as_u64().map(|n| n as usize),
            Value::String(s) => s.trim().parse().ok(),
            // no ids at all: answers in order
            Value::Null => items.get(pos).map(|i| i.id),
            _ => None,
        };
        let Some(item) = id.and_then(|id| items.iter().find(|i| i.id == id)) else { continue };
        let t = clean(&e["translation"], &item.text);
        let r = clean(&e["romanization"], &item.text).filter(|r| script::detect(r) == "latn" || script::detect(r).is_empty());
        out.insert(item.id, (t, r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        vec![
            Item { id: 1, kind: Kind::Title, text: "夜に駆ける".into(), artist: Some("YOASOBI".into()) },
            Item { id: 2, kind: Kind::Artist, text: "ヨルシカ".into(), artist: None },
            Item { id: 3, kind: Kind::Album, text: "Кино".into(), artist: Some("Кино".into()) },
        ]
    }

    #[test]
    fn prompt_has_the_items_and_rules() {
        let m = messages(&items(), "en", true);
        let sys = m[0]["content"].as_str().unwrap();
        assert!(sys.contains("into English") && sys.contains("Hepburn") && sys.contains("JSON only"));
        let user = m[1]["content"].as_str().unwrap();
        assert!(user.ends_with("/no_think"));
        let v: Value = serde_json::from_str(user.trim_end_matches("/no_think").trim()).unwrap();
        assert_eq!(v["items"][0]["kind"], "song title");
        assert_eq!(v["items"][0]["artist"], "YOASOBI");
        assert!(v["items"][2].get("artist").is_none(), "an artist equal to the text adds nothing");
        assert!(!messages(&items(), "de", false)[1]["content"].as_str().unwrap().contains("no_think"));
        assert!(system_prompt("de").contains("into German"));
    }

    #[test]
    fn parses_plain_json() {
        let r = parse(r#"{"items":[{"id":1,"translation":"Racing Into the Night","romanization":"Yoru ni Kakeru"},{"id":2,"translation":null,"romanization":"Yorushika"}]}"#, &items());
        assert_eq!(r[&1], (Some("Racing Into the Night".into()), Some("Yoru ni Kakeru".into())));
        assert_eq!(r[&2], (None, Some("Yorushika".into())));
        assert!(!r.contains_key(&3));
    }

    #[test]
    fn parses_around_thinking_and_fences() {
        let reply = "<think>\nok so the title means…\n</think>\n```json\n[{\"id\": \"3\", \"translation\": \"Cinema\", \"romanization\": \"Kino\"}]\n```";
        let r = parse(reply, &items());
        assert_eq!(r[&3], (Some("Cinema".into()), Some("Kino".into())));
        // an unfinished think block and nothing else
        assert!(parse("<think>hmm", &items()).is_empty());
        assert!(parse("Sorry, I can't.", &items()).is_empty());
    }

    #[test]
    fn drops_repeats_nulls_and_unknown_ids() {
        let reply = r#"{"items":[{"id":1,"translation":"夜に駆ける","romanization":"null"},{"id":9,"translation":"x"},{"id":2,"translation":"","romanization":"ヨルシカ"}]}"#;
        let r = parse(reply, &items());
        assert_eq!(r[&1], (None, None), "the original repeated is not a translation");
        assert_eq!(r[&2], (None, None), "a romanization must be in Latin letters");
        assert!(!r.contains_key(&9));
        // answers without ids are taken in order
        let r = parse(r#"[{"translation":"Racing Into the Night"},{"romanization":"Yorushika"}]"#, &items());
        assert_eq!(r[&1].0.as_deref(), Some("Racing Into the Night"));
        assert_eq!(r[&2].1.as_deref(), Some("Yorushika"));
    }
}
