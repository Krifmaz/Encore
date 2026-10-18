//! Formatting helpers shared by every page: sizes, durations and dates in the same
//! words the web app uses.

use serde_json::Value;

/// `1.2 GB`, `340 MB`, `12 kB`.
pub fn size(bytes: f64) -> String {
    let units = ["B", "kB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut i = 0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", v as u64)
    } else if v >= 100.0 {
        format!("{} {}", v.round() as u64, units[i])
    } else {
        format!("{v:.1} {}", units[i])
    }
}

/// `3:07`, `1:02:15`.
pub fn mmss(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// `2 h 5 min`, `48 min`, `12 songs` style totals for album and playlist headers.
pub fn duration_long(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}


/// `3 min ago`, `yesterday`, `12 Mar` for a unix timestamp in seconds.
pub fn ago(ts: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let d = (now - ts).max(0.0);
    if d < 60.0 {
        "just now".into()
    } else if d < 3600.0 {
        format!("{} min ago", (d / 60.0) as u64)
    } else if d < 86_400.0 {
        format!("{} h ago", (d / 3600.0) as u64)
    } else if d < 2.0 * 86_400.0 {
        "yesterday".into()
    } else if d < 30.0 * 86_400.0 {
        format!("{} days ago", (d / 86_400.0) as u64)
    } else {
        date(ts)
    }
}

/// `12 Mar 2026` for a unix timestamp in seconds (UTC).
pub fn date(ts: f64) -> String {
    let days = (ts / 86_400.0).floor() as i64;
    let (y, m, d) = civil_from_days(days);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// Days since 1970-01-01 to (year, month, day); Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The quality label the web shows for a track: `FLAC`, `MP3 320`, `AAC 256`.
pub fn quality(track: &Value) -> String {
    let format = track
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    let bitrate = track.get("bitrate").and_then(Value::as_f64).unwrap_or(0.0);
    match format.as_str() {
        "" => String::new(),
        "FLAC" | "ALAC" | "WAV" | "AIFF" | "APE" | "WV" | "DSF" | "DFF" => format,
        _ if bitrate > 0.0 => format!("{format} {}", bitrate.round() as u64),
        _ => format,
    }
}

/// `s(v, "key")`: a JSON string field or "".
pub fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// `f(v, "key")`: a JSON number field or 0.
pub fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `i(v, "key")`: a JSON integer field or 0.
pub fn i(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
        .unwrap_or(0)
}

/// `truthy(v, "key")`: JavaScript truthiness of a field.
pub fn truthy(v: &Value, key: &str) -> bool {
    match v.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// The rows of a JSON answer that is either a list or `{items: [...]}`.
pub fn items(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("items")
            .or_else(|| o.get("results"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_read_like_the_web() {
        assert_eq!(size(0.0), "0 B");
        assert_eq!(size(12_300.0), "12.3 kB");
        assert_eq!(size(335_082.0), "335 kB");
        assert_eq!(size(1.2e9), "1.2 GB");
        assert_eq!(mmss(187.0), "3:07");
        assert_eq!(mmss(3735.0), "1:02:15");
        assert_eq!(duration_long(7500.0), "2 h 5 min");
        assert_eq!(date(1_792_311_210.0), "18 Oct 2026");
    }

    #[test]
    fn json_helpers_are_forgiving() {
        let v = serde_json::json!({ "a": "x", "n": 3, "b": true, "list": [1], "fmt": "mp3", "bitrate": 320 });
        assert_eq!(s(&v, "a"), "x");
        assert_eq!(s(&v, "n"), "3");
        assert_eq!(i(&v, "n"), 3);
        assert!(truthy(&v, "b") && truthy(&v, "list") && !truthy(&v, "missing"));
        assert_eq!(items(&serde_json::json!({ "items": [1, 2] })).len(), 2);
        let t = serde_json::json!({ "format": "mp3", "bitrate": 320 });
        assert_eq!(quality(&t), "MP3 320");
        assert_eq!(quality(&serde_json::json!({ "format": "flac", "bitrate": 900 })), "FLAC");
    }
}
