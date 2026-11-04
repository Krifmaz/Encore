//! Encore's server state handed to the Krifmaz kit's server crates (`kit-server-*`): they use Encore's
//! database and Encore's sign-in check (cookie session with its CSRF header, app token or API key), so a route
//! served by a kit crate answers exactly as Encore's own did.
use crate::auth::{Admin, User};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use kit_server_core::{Account, Db, Host};

impl Host for AppState {
    fn db(&self) -> &dyn Db {
        &self.db
    }

    async fn signed_in(&self, parts: &mut Parts) -> kit_server_core::Result<Account> {
        let User(u) = User::from_request_parts(parts, self).await?;
        Ok(Account::from_row(u))
    }

    async fn admin(&self, parts: &mut Parts) -> kit_server_core::Result<Account> {
        let Admin(u) = Admin::from_request_parts(parts, self).await?;
        Ok(Account::from_row(u))
    }
}
