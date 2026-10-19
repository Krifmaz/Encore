//! A small context for this crate's tests: the settings (on a database in a temporary folder) and the
//! HTTP client – what the download clients and the other services need ([`encore_http::Ctx`]). Tests
//! that need a whole server run in crates/api on its TestServer.
use encore_config::Config;
use encore_db::Db;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct TestCtx {
    pub cfg: Config,
    pub dir: PathBuf,
    http: reqwest::Client,
    http_no_redirects: reqwest::Client,
    pub urls: encore_http::Urls,
    mb: encore_http::mb::Mb,
    pace: encore_http::Pace,
    cache: Mutex<HashMap<String, (Instant, Value)>>,
}

static SEQ: AtomicUsize = AtomicUsize::new(0);

impl TestCtx {
    /// A new, empty folder under the system's temp folder (named after `name`); removed when dropped.
    pub fn new(name: &str) -> TestCtx {
        let dir = std::env::temp_dir().join(format!("encore-requests-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = Config::load(Db::open(&dir).unwrap()).unwrap();
        let http = encore_http::client::build("").unwrap();
        let http_no_redirects = encore_http::client::no_redirects("").unwrap();
        TestCtx { cfg, dir, http, http_no_redirects, urls: Default::default(), mb: Default::default(), pace: Default::default(), cache: Default::default() }
    }
}

impl Drop for TestCtx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl encore_http::Ctx for TestCtx {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }
    fn http_no_redirects(&self) -> &reqwest::Client {
        &self.http_no_redirects
    }
    fn urls(&self) -> &encore_http::Urls {
        &self.urls
    }
    fn cfg(&self) -> &Config {
        &self.cfg
    }
    fn mb(&self) -> &encore_http::mb::Mb {
        &self.mb
    }
    fn pace(&self) -> &encore_http::Pace {
        &self.pace
    }
    fn cache_get(&self, key: &str) -> Option<Value> {
        self.cache.lock().unwrap().get(key).filter(|(exp, _)| *exp > Instant::now()).map(|(_, v)| v.clone())
    }
    fn cache_put(&self, key: &str, v: Value, secs: u64) {
        self.cache.lock().unwrap().insert(key.to_string(), (Instant::now() + Duration::from_secs(secs), v));
    }
}
