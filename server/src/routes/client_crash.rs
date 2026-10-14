//! App crash reports (Diagnostics → "App crashes"). The Android app writes a report when it crashes (Java/Kotlin
//! stack, or a native crash / ANR / low-memory kill read back from the system on the next start) and uploads it
//! here the next time it runs. Admins see the newest 50.
//!
//! POST   /api/diagnostics/client-crash  {kind, device, device_id, app_version, os, time, summary, text, key} → {ok, id, duplicate}
//! GET    /api/diagnostics/client-crash  (admin) → {crashes: [{id, kind, user, device, device_id, app_version, os, time, received_at, first_line, text}]}
//! DELETE /api/diagnostics/client-crash  (admin) → {ok}
use crate::auth::{Admin, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::p;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// How many reports are kept (the oldest go first).
pub const KEEP: i64 = 50;
/// A report's text is cut to this many bytes (an ANR trace holds every thread of the process).
const MAX_TEXT: usize = 256 * 1024;

fn ensure_table(st: &AppState) -> ApiResult<()> {
    // created on first use: an older server simply never reads it
    st.db.execute_batch(
        "CREATE TABLE IF NOT EXISTS client_crashes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER,
            username TEXT,
            kind TEXT,
            device TEXT,
            device_id TEXT,
            app_version TEXT,
            os TEXT,
            time REAL,
            received_at REAL,
            first_line TEXT,
            text TEXT,
            dedupe TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_client_crashes_dedupe ON client_crashes(dedupe);",
    )?;
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct CrashBody {
    kind: String,
    device: String,
    device_id: String,
    app_version: String,
    os: String,
    /// when it happened (epoch seconds); 0 = when it arrived
    time: f64,
    /// the exception line ("java.lang.IllegalStateException: …"); empty = the first line of `text`
    summary: String,
    text: String,
    /// the same crash sent twice (an upload whose answer got lost) is kept once: per device and key
    key: String,
}

/// `s` cut to at most `max` bytes, on a character boundary.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut at {} KB)", &s[..end], max / 1024)
}

/// The line shown in the list: the summary, else the first non-empty line of the text.
pub fn first_line(summary: &str, text: &str) -> String {
    let line = if summary.trim().is_empty() { text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("") } else { summary.trim() };
    line.chars().take(300).collect()
}

pub async fn upload(State(st): State<AppState>, User(u): User, Json(b): Json<CrashBody>) -> ApiResult<Json<Value>> {
    if b.text.trim().is_empty() && b.summary.trim().is_empty() {
        return Err(err(400, "Empty crash report"));
    }
    ensure_table(&st)?;
    let kind = if b.kind.trim().is_empty() { "crash".to_string() } else { b.kind.trim().chars().take(40).collect() };
    let small = |s: &str| s.trim().chars().take(200).collect::<String>();
    let received = now();
    let time = if b.time > 0.0 && b.time.is_finite() { b.time } else { received };
    let dedupe = if b.key.trim().is_empty() { None } else { Some(format!("{}|{}|{}", u.i("id"), small(&b.device_id), small(&b.key))) };
    if let Some(d) = &dedupe {
        if let Some(r) = st.db.one("SELECT id FROM client_crashes WHERE dedupe=?", p![d.clone()])? {
            return Ok(Json(json!({ "ok": true, "id": r.i("id"), "duplicate": true })));
        }
    }
    let id = st.db.execute(
        "INSERT INTO client_crashes (user_id, username, kind, device, device_id, app_version, os, time, received_at, first_line, text, dedupe) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![u.i("id"), u.s("username"), kind, small(&b.device), small(&b.device_id), small(&b.app_version), small(&b.os), time, received,
           first_line(&b.summary, &b.text), cut(&b.text, MAX_TEXT), dedupe],
    )?;
    st.db.execute("DELETE FROM client_crashes WHERE id NOT IN (SELECT id FROM client_crashes ORDER BY id DESC LIMIT ?)", p![KEEP])?;
    tracing::warn!("app crash report from {} ({}): {}", u.s("username"), small(&b.device), first_line(&b.summary, &b.text));
    Ok(Json(json!({ "ok": true, "id": id, "duplicate": false })))
}

pub async fn list(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let rows = st.db.query(
        "SELECT id, kind, username user, device, device_id, app_version, os, time, received_at, first_line, text FROM client_crashes ORDER BY time DESC, id DESC LIMIT ?",
        p![KEEP],
    )?;
    Ok(Json(json!({ "crashes": rows })))
}

pub async fn clear(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    st.db.execute("DELETE FROM client_crashes", p![])?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
#[path = "client_crash_tests.rs"]
mod tests;
