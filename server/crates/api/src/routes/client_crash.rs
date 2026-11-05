//! App crash reports (Diagnostics → "App crashes"), served by the Krifmaz kit's `kit-server-bugreports::crash`.
//! The Android app writes a report when it crashes (Java/Kotlin stack, or a native crash / ANR / low-memory kill
//! read back from the system on the next start) and uploads it here the next time it runs. Admins see the newest 50.
//! The route stays where stats.rs registers it; these handlers are the kit's, for Encore's state.
//!
//! POST   /api/diagnostics/client-crash  {kind, device, device_id, app_version, os, time, summary, text, key} → {ok, id, duplicate}
//! GET    /api/diagnostics/client-crash  (admin) → {crashes: [{id, kind, user, device, device_id, app_version, os, time, received_at, first_line, text}]}
//! DELETE /api/diagnostics/client-crash  (admin) → {ok}
use crate::state::AppState;
use axum::Router;
use kit_server_bugreports::crash::{self, CrashConfig, CrashHost, CrashPaths};
use kit_server_bugreports::CrashStore;

#[cfg(test)]
use kit_server_bugreports::crash::first_line;

/// How many crash reports are kept, the oldest going first.
pub const KEEP: i64 = 50;

/// The crash report route (POST, GET, DELETE) at Encore's path.
pub fn router() -> Router<AppState> {
    crash::router_with(CrashConfig {
        paths: CrashPaths {
            crashes: "/api/diagnostics/client-crash".into(),
        },
        keep: KEEP,
        ..CrashConfig::default()
    })
}

/// The crash reports are kept with the bug reports, in the kit's tables of Encore's database.
impl CrashHost for AppState {
    fn crashes(&self) -> &dyn CrashStore {
        &self.kit.reports
    }
}

#[cfg(test)]
#[path = "client_crash_tests.rs"]
mod tests;
