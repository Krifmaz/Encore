use crate::auth::RateLimiter;
use crate::config::Config;
use crate::db::Db;
use crate::library::Library;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Config,
    pub http: reqwest::Client,
    pub limits: Arc<RateLimiter>,
    pub static_dir: PathBuf,
    pub library: Library,
}

impl AppState {
    pub fn data_dir(&self) -> &std::path::Path {
        &self.db.data_dir
    }
}
