use crate::auth::RateLimiter;
use crate::config::Config;
use crate::db::Db;
use crate::library::Library;
use crate::mb::Mb;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
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
    pub mb: Mb,
    pub cache: Arc<Mutex<HashMap<String, (Instant, Value)>>>,
}

impl AppState {
    pub fn data_dir(&self) -> &std::path::Path {
        &self.db.data_dir
    }

    /// Small in-memory cache for slow lookups (Deezer, Last.fm …).
    pub fn cache_get(&self, key: &str) -> Option<Value> {
        let c = self.cache.lock().unwrap();
        c.get(key).filter(|(exp, _)| *exp > Instant::now()).map(|(_, v)| v.clone())
    }
    pub fn cache_put(&self, key: &str, v: Value, secs: u64) {
        let mut c = self.cache.lock().unwrap();
        if c.len() > 5000 {
            let now = Instant::now();
            c.retain(|_, (exp, _)| *exp > now);
        }
        c.insert(key.to_string(), (Instant::now() + Duration::from_secs(secs), v));
    }
}
