//! Reading loosely typed JSON answers (MusicBrainz, Deezer, Apple, media servers) as text.
use serde_json::Value;

/// A JSON string's text; anything else (a number, null, a missing field) is empty.
pub fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// [`s`] without the copy: the string inside the value, or "".
pub fn str_ref(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// Any JSON value as text: a string as it is, null as nothing, numbers and the rest as their JSON
/// (an id that one service sends as a number and another as a string reads the same).
pub fn text(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn strings_and_the_rest() {
        assert_eq!(super::s(&json!("a")), "a");
        assert_eq!(super::s(&json!(12)), "");
        assert_eq!(super::s(&json!(null)), "");
        assert_eq!(super::str_ref(&json!("a")), "a");
        assert_eq!(super::str_ref(&json!([1])), "");
        assert_eq!(super::text(&json!("a")), "a");
        assert_eq!(super::text(&json!(12)), "12");
        assert_eq!(super::text(&json!(null)), "");
        assert_eq!(super::text(&json!(true)), "true");
    }
}
