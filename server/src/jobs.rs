//! Progress of long-running library jobs (shown in Settings → Library).
use serde_json::{json, Map, Value};
use std::sync::{LazyLock, Mutex};

static JOBS: LazyLock<Mutex<Map<String, Value>>> = LazyLock::new(|| Mutex::new(Map::new()));

pub fn get(job: &str) -> Value {
    JOBS.lock().unwrap().get(job).cloned().unwrap_or(json!({ "running": false }))
}

pub fn running(job: &str) -> bool {
    get(job)["running"].as_bool().unwrap_or(false)
}

/// Merge fields into a job's state.
pub fn set(job: &str, patch: Value) {
    let mut j = JOBS.lock().unwrap();
    let cur = j.entry(job.to_string()).or_insert(json!({ "running": false }));
    if let (Some(c), Value::Object(p)) = (cur.as_object_mut(), patch) {
        for (k, v) in p {
            c.insert(k, v);
        }
    }
}

/// Add n to a counter.
pub fn add(job: &str, key: &str, n: i64) {
    let mut j = JOBS.lock().unwrap();
    let cur = j.entry(job.to_string()).or_insert(json!({ "running": false }));
    cur[key] = json!(cur[key].as_i64().unwrap_or(0) + n);
}

/// Claim a job; false when it's already running.
pub fn start(job: &str, init: Value) -> bool {
    let mut j = JOBS.lock().unwrap();
    if j.get(job).is_some_and(|v| v["running"].as_bool().unwrap_or(false)) {
        return false;
    }
    let mut v = init;
    v["running"] = json!(true);
    j.insert(job.to_string(), v);
    true
}
