//! Progress of long-running library jobs (shown in Settings → Library).
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};

/// Each job's state by name (`{"running": bool, …counters}`). Clones share the same table.
#[derive(Clone, Default)]
pub struct Jobs(Arc<Mutex<Map<String, Value>>>);

impl Jobs {
    /// The state of `job` (`{"running": false}` when it never ran).
    pub fn get(&self, job: &str) -> Value {
        self.0.lock().unwrap().get(job).cloned().unwrap_or(json!({ "running": false }))
    }

    /// Whether `job` is running now.
    pub fn running(&self, job: &str) -> bool {
        self.get(job)["running"].as_bool().unwrap_or(false)
    }

    /// Merge fields into a job's state.
    pub fn set(&self, job: &str, patch: Value) {
        let mut j = self.0.lock().unwrap();
        let cur = j.entry(job.to_string()).or_insert(json!({ "running": false }));
        if let (Some(c), Value::Object(p)) = (cur.as_object_mut(), patch) {
            for (k, v) in p {
                c.insert(k, v);
            }
        }
    }

    /// Add n to a counter.
    pub fn add(&self, job: &str, key: &str, n: i64) {
        let mut j = self.0.lock().unwrap();
        let cur = j.entry(job.to_string()).or_insert(json!({ "running": false }));
        cur[key] = json!(cur[key].as_i64().unwrap_or(0) + n);
    }

    /// Claim a job; false when it's already running.
    pub fn start(&self, job: &str, init: Value) -> bool {
        let mut j = self.0.lock().unwrap();
        if j.get(job).is_some_and(|v| v["running"].as_bool().unwrap_or(false)) {
            return false;
        }
        let mut v = init;
        v["running"] = json!(true);
        j.insert(job.to_string(), v);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_runs_once_at_a_time() {
        let jobs = Jobs::default();
        assert!(!jobs.running("scan"));
        assert!(jobs.start("scan", json!({ "done": 0 })));
        assert!(!jobs.start("scan", json!({})), "already running");
        jobs.add("scan", "done", 2);
        jobs.set("scan", json!({ "current": "x" }));
        assert_eq!(jobs.get("scan"), json!({ "running": true, "done": 2, "current": "x" }));
        jobs.set("scan", json!({ "running": false }));
        assert!(jobs.start("scan", json!({})));
        assert!(!Jobs::default().running("scan"), "each server has its own table");
    }
}
