//! App crash reports (Diagnostics → "App crashes"), served by the Krifmaz kit's `kit-server-bugreports::crash`.
//! The Android app writes a report when it crashes (Java/Kotlin stack, or a native crash / ANR / low-memory kill
//! read back from the system on the next start) and uploads it here the next time it runs. Admins see the newest 50.
//! The route stays where stats.rs registers it; these handlers are the kit's, for Encore's state.
//!
//! POST   /api/diagnostics/client-crash  {kind, device, device_id, app_version, os, time, summary, text, key} → {ok, id, duplicate}
//! GET    /api/diagnostics/client-crash  (admin) → {crashes: [{id, kind, user, device, device_id, app_version, os, time, received_at, first_line, text}]}
//! DELETE /api/diagnostics/client-crash  (admin) → {ok}
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use kit_server_bugreports::crash;
use kit_server_core::{Admin, Result, SignedIn};
use serde_json::Value;

pub use kit_server_bugreports::crash::CrashBody;
#[cfg(test)]
use kit_server_bugreports::crash::{first_line, KEEP};

/// POST /api/diagnostics/client-crash: keep a crash report from an app.
pub async fn upload(st: State<AppState>, u: SignedIn, b: Json<CrashBody>) -> Result<Json<Value>> {
    crash::upload(st, u, b).await
}

/// GET /api/diagnostics/client-crash: the crash reports, newest first (admins).
pub async fn list(st: State<AppState>, a: Admin) -> Result<Json<Value>> {
    crash::list(st, a).await
}

/// DELETE /api/diagnostics/client-crash: delete every crash report (admins).
pub async fn clear(st: State<AppState>, a: Admin) -> Result<Json<Value>> {
    crash::clear(st, a).await
}

#[cfg(test)]
#[path = "client_crash_tests.rs"]
mod tests;
