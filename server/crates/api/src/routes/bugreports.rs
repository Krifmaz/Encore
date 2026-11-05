//! Bug reports (side bar → "Bug reports"), served by the Krifmaz kit's `kit-server-bugreports`. A user writes what
//! went wrong, attaches screenshots or a log, and the admin answers in a thread; the admin can export the lot as a
//! zip (or one plain text file) to hand to the developer. The routes, limits and words are the kit crate's; this
//! file hands it Encore's pieces: the store (the kit's tables in Encore's database), the config folder, the name and
//! version, the rate limiter, the activity log, the Discord notification, the reply mail and the upload decode
//! slots.
//!
//! POST   /api/bug-reports                      multipart kind, title, body, page, app_version, device, file… → {ok, id}
//! GET    /api/bug-reports?status=&kind=&q=&mine=1 → {reports: […]}
//! GET    /api/bug-reports/unread               → {count}
//! GET    /api/bug-reports/export?status=&kind=&ids=  (admin) → application/zip; …&format=txt → text/plain
//! GET    /api/bug-reports/{id}                 → {report: {…, files: […]}, messages: [{…, files: […]}]}
//! POST   /api/bug-reports/{id}/messages        multipart body, file… → {ok, id}
//! POST   /api/bug-reports/{id}/status          {status} (admin) → {ok}
//! DELETE /api/bug-reports/{id}                 (admin) → {ok}
//! GET    /api/bug-reports/{id}/files/{fid}     → the file
use crate::email;
use crate::state::AppState;
use axum::Router;
use encore_requests::notify;
use kit_server_bugreports::{BugReportStore, BugReportsHost};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

/// The bug report routes.
pub fn router() -> Router<AppState> {
    kit_server_bugreports::router()
}

impl BugReportsHost for AppState {
    fn reports(&self) -> &dyn BugReportStore {
        &self.kit.reports
    }

    fn data_dir(&self) -> PathBuf {
        AppState::data_dir(self).to_path_buf()
    }

    fn app_name(&self) -> String {
        "Encore".to_string()
    }

    fn app_version(&self) -> String {
        crate::VERSION.to_string()
    }

    fn limited(&self, key: &str, limit: usize, window_secs: f64) -> bool {
        self.limits.limited(key, limit, window_secs, true)
    }

    fn log(&self, level: &str, message: &str) {
        self.db.log(level, message);
    }

    fn notify(&self, title: String, message: String) -> impl Future<Output = ()> + Send + 'static {
        let st = self.clone();
        async move { notify::send(&st, &title, &message, None).await }
    }

    fn mail_enabled(&self) -> bool {
        email::enabled(self)
    }

    fn base_url(&self) -> String {
        self.cfg.str("base_url")
    }

    fn send_mail(
        &self,
        to: String,
        subject: String,
        text: String,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let st = self.clone();
        async move {
            email::send(&st, &to, &subject, &text)
                .await
                .map_err(|e| e.to_string())
        }
    }

    fn decodes(&self) -> Arc<tokio::sync::Semaphore> {
        self.web.decodes.clone()
    }
}

#[cfg(test)]
#[path = "bugreports_tests.rs"]
mod tests;
