//! JavaScript's numbers, in Rust: the string a number prints as, `Number(x)` of a string or a JSON value,
//! `Math.round`, `toFixed(1)`, `toString(36)`. The player, the playback settings and the remote all need the
//! same few helpers, so they live here once (the pages still carry a few local variants of their own).

use serde_json::{json, Value};

/// `${n}` of a JS number ("0.9", "1", "52.3").
pub fn js_num(v: f64) -> String { v.to_string() }
/// `Number("…")` of an input value (0 when it isn't a number).
pub fn num(s: &str) -> f64 { s.trim().parse().unwrap_or(0.0) }
/// JS truthiness of a number.
pub fn truthy_num(x: f64) -> bool { x != 0.0 && !x.is_nan() }
/// A JSON number the way JS would print it (whole numbers without ".0").
pub fn jnum(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 { json!(x as i64) } else { json!(x) }
}
/// `Math.round(x)` (halves round up, also for negatives).
pub fn js_round(x: f64) -> f64 { (x + 0.5).floor() }
/// `(x).toFixed(1)` – exact decimal expansion, ties (k.25 / k.75) rounded up like JS.
pub fn to_fixed1(x: f64) -> String {
    let q = x * 4.0;
    if x.is_finite() && q.fract() == 0.0 && (q as i64).rem_euclid(2) == 1 {
        let n = (x * 10.0).round() as i64;
        format!("{}.{}", n / 10, (n % 10).abs())
    } else {
        format!("{:.1}", x)
    }
}
/// `String(Math.round(hz / 100) / 10)` – a sample rate in kHz ("44.1", "48", "192").
pub fn khz(hz: f64) -> String {
    // exact for every common rate: 44.1, 48, 22.05, 11.025 (up to three decimals, no trailing zeros)
    let s = format!("{:.3}", hz / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// JavaScript's `Number(x)` for a JSON value (NaN where JS gives NaN).
pub fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => { let t = s.trim(); if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) } }
        Value::Array(a) => match a.as_slice() { [] => 0.0, [x] if !x.is_array() && !x.is_object() => js_number(x), _ => f64::NAN },
        Value::Object(_) => f64::NAN,
    }
}

/// `n.toString(36)`.
pub fn to36(n: f64) -> String { js_sys::Number::from(n).to_string_with_radix(36).map(String::from).unwrap_or_default() }
/// `Math.random().toString(36).slice(2)`.
pub fn rand36() -> String {
    let s = js_sys::JsString::from(to36(js_sys::Math::random()));
    String::from(s.slice(2, s.length()))
}
