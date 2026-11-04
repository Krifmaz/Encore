//! Encore's own updates (Settings → Diagnostics → Updates):
//!   GET  /api/system/update[?force=1]  (admin) → {current, latest, is_newer, notes, url, published_at, checked_at,
//!        restart: "ready", self_update: "ready" | "unavailable", self_update_reason, image, …}
//!   POST /api/system/update {tag?}     (admin) → 202: the new image is pulled and the helper container started
//!   POST /api/system/restart           (admin) → 202: the server stops gracefully and exits with code 75
//! The routes are the kit's (`kit_server_updates::router`, merged in routes::system); what they do is
//! encore_system::update's, through the kit's `Updates` trait implemented here.
use crate::state::AppState;
use encore_core::error::err;
use encore_system::update;
use kit_server_core::Account;
use serde_json::Value;

impl kit_server_updates::Updates for AppState {
    /// The update card (`force`: Check now).
    async fn update_status(&self, force: bool) -> kit_server_core::Result<Value> {
        Ok(update::status(self, force).await)
    }

    /// Update now: the new image pulled and the helper container started; Encore's refusals as they are.
    async fn update_now(&self, who: &Account, tag: Option<String>) -> kit_server_core::Result<Value> {
        match update::update_now(self, &who.username, tag).await {
            Ok(v) => Ok(v),
            Err(update::Refused(code, why)) => Err(err(code, why).into()),
        }
    }

    /// Restart server: half a second later, so the 202 answer gets out first.
    fn restart(&self, who: &Account) {
        update::restart(self, &who.username);
    }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
