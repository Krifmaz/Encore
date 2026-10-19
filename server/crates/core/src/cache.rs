//! A cache held to a number of bytes: when a new answer would make it bigger than `max_bytes`, the answers
//! used longest ago leave first (least recently used). Each entry may also have an end of life (a time to
//! live) and a stamp the reader compares (a library version, say), so a cache never answers with something
//! older than its owner allows.
//!
//! The server keeps a few of these: the library list answers, the answers of other services (Deezer, Last.fm,
//! MusicBrainz …). `/api/admin/perf` shows their sizes ([`ByteCache::stats`]).
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry<V> {
    value: V,
    bytes: usize,
    expires: Option<Instant>,
    stamp: u64,
    /// when it was last used (a counter, not a clock): the key of `Inner::order`
    used: u64,
}

struct Inner<V> {
    map: HashMap<String, Entry<V>>,
    /// last use → key: the first one is the least recently used
    order: BTreeMap<u64, String>,
    bytes: usize,
    tick: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

/// What a cache holds and how it went, for `/api/admin/perf`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub max_bytes: usize,
    pub max_entries: usize,
    pub hits: u64,
    pub misses: u64,
    /// entries dropped to make room (not those that ran out of time or were replaced)
    pub evictions: u64,
}

/// See the module. Values are cloned out on a hit, so a large value is best kept behind an `Arc`.
pub struct ByteCache<V> {
    inner: Mutex<Inner<V>>,
    max_bytes: usize,
    max_entries: usize,
}

impl<V: Clone> ByteCache<V> {
    /// At most `max_bytes` (as the caller counts them on `insert`) and at most `max_entries` entries.
    pub fn new(max_bytes: usize, max_entries: usize) -> Self {
        ByteCache {
            inner: Mutex::new(Inner { map: HashMap::new(), order: BTreeMap::new(), bytes: 0, tick: 0, hits: 0, misses: 0, evictions: 0 }),
            max_bytes,
            max_entries: max_entries.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<V>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The value for `key` while it lives.
    pub fn get(&self, key: &str) -> Option<V> {
        self.get_if(key, |_| true)
    }

    /// The value for `key` while it lives and `fresh(its stamp)` says it may be used. An entry that has run
    /// out of time is dropped; one whose stamp is refused stays (the owner may replace it).
    pub fn get_if(&self, key: &str, fresh: impl FnOnce(u64) -> bool) -> Option<V> {
        let mut g = self.lock();
        let inner = &mut *g;
        let now = Instant::now();
        let Some(e) = inner.map.get_mut(key) else {
            inner.misses += 1;
            return None;
        };
        if e.expires.is_some_and(|t| t <= now) {
            let used = e.used;
            let bytes = e.bytes;
            inner.map.remove(key);
            inner.order.remove(&used);
            inner.bytes -= bytes;
            inner.misses += 1;
            return None;
        }
        if !fresh(e.stamp) {
            inner.misses += 1;
            return None;
        }
        inner.tick += 1;
        inner.order.remove(&e.used);
        e.used = inner.tick;
        inner.order.insert(e.used, key.to_string());
        inner.hits += 1;
        Some(e.value.clone())
    }

    /// Keeps `value` (taking `bytes`) under `key` for `ttl` (None: until it is pushed out), with `stamp`. A
    /// value bigger than the whole cache is not kept. Expired entries go first, then the least recently used
    /// ones until the new one fits.
    pub fn insert(&self, key: impl Into<String>, value: V, bytes: usize, ttl: Option<Duration>, stamp: u64) {
        let key = key.into();
        let mut g = self.lock();
        let inner = &mut *g;
        if let Some(old) = inner.map.remove(&key) {
            inner.order.remove(&old.used);
            inner.bytes -= old.bytes;
        }
        if bytes > self.max_bytes {
            return;
        }
        if inner.bytes + bytes > self.max_bytes || inner.map.len() >= self.max_entries {
            let now = Instant::now();
            let expired: Vec<String> = inner.map.iter().filter(|(_, e)| e.expires.is_some_and(|t| t <= now)).map(|(k, _)| k.clone()).collect();
            for k in expired {
                if let Some(e) = inner.map.remove(&k) {
                    inner.order.remove(&e.used);
                    inner.bytes -= e.bytes;
                }
            }
        }
        while inner.bytes + bytes > self.max_bytes || inner.map.len() >= self.max_entries {
            let Some((_, k)) = inner.order.pop_first() else { break };
            if let Some(e) = inner.map.remove(&k) {
                inner.bytes -= e.bytes;
                inner.evictions += 1;
            }
        }
        inner.tick += 1;
        let used = inner.tick;
        inner.order.insert(used, key.clone());
        inner.bytes += bytes;
        inner.map.insert(key, Entry { value, bytes, expires: ttl.map(|t| Instant::now() + t), stamp, used });
    }

    /// Drops every entry for which `keep(stamp)` says no.
    pub fn retain(&self, mut keep: impl FnMut(u64) -> bool) {
        let mut g = self.lock();
        let inner = &mut *g;
        let gone: Vec<String> = inner.map.iter().filter(|(_, e)| !keep(e.stamp)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            if let Some(e) = inner.map.remove(&k) {
                inner.order.remove(&e.used);
                inner.bytes -= e.bytes;
            }
        }
    }

    /// Empties it (after a database restore).
    pub fn clear(&self) {
        let mut g = self.lock();
        g.map.clear();
        g.order.clear();
        g.bytes = 0;
    }

    /// The keys it holds now (for tests and the perf page).
    pub fn keys(&self) -> Vec<String> {
        self.lock().map.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stats(&self) -> CacheStats {
        let g = self.lock();
        CacheStats { entries: g.map.len(), bytes: g.bytes, max_bytes: self.max_bytes, max_entries: self.max_entries, hits: g.hits, misses: g.misses, evictions: g.evictions }
    }
}

/// About how many bytes of memory a JSON value holds (the value nodes, the text of strings and keys): what the
/// caches count their answers by. Cheap next to building the answer (one walk, no copying).
pub fn json_bytes(v: &Value) -> usize {
    const NODE: usize = std::mem::size_of::<Value>();
    match v {
        Value::String(s) => NODE + s.len(),
        Value::Array(a) => NODE + a.iter().map(json_bytes).sum::<usize>(),
        // a map entry: the key's String and its text, the value, the map's own bookkeeping
        Value::Object(o) => NODE + o.iter().map(|(k, x)| std::mem::size_of::<String>() + k.len() + 16 + json_bytes(x)).sum::<usize>(),
        _ => NODE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_least_recently_used_leave_first() {
        let c: ByteCache<i32> = ByteCache::new(100, 10);
        c.insert("a", 1, 40, None, 0);
        c.insert("b", 2, 40, None, 0);
        // a is used: b is now the oldest
        assert_eq!(c.get("a"), Some(1));
        c.insert("c", 3, 40, None, 0);
        assert_eq!(c.get("b"), None, "b made room");
        assert_eq!((c.get("a"), c.get("c")), (Some(1), Some(3)));
        let s = c.stats();
        assert_eq!((s.entries, s.bytes, s.evictions), (2, 80, 1));
        // too big for the whole cache: not kept, nothing pushed out
        c.insert("huge", 9, 101, None, 0);
        assert_eq!((c.get("huge"), c.len()), (None, 2));
        // replacing a key counts its bytes once
        c.insert("a", 10, 10, None, 0);
        assert_eq!((c.get("a"), c.stats().bytes), (Some(10), 50));
    }

    #[test]
    fn entries_are_bounded_by_count_too() {
        let c: ByteCache<i32> = ByteCache::new(1 << 20, 3);
        for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
            c.insert(*k, i as i32, 1, None, 0);
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("d"), Some(3));
    }

    #[test]
    fn time_and_stamps_decide_what_may_be_used() {
        let c: ByteCache<&'static str> = ByteCache::new(1000, 10);
        c.insert("gone", "x", 10, Some(Duration::from_millis(0)), 0);
        c.insert("kept", "y", 10, Some(Duration::from_secs(60)), 7);
        assert_eq!(c.get("gone"), None);
        assert_eq!(c.stats().bytes, 10, "the expired entry was dropped on the look");
        assert_eq!(c.get_if("kept", |s| s == 7), Some("y"));
        assert_eq!(c.get_if("kept", |s| s == 8), None, "another stamp: not used");
        assert_eq!(c.len(), 1, "but kept for its owner to replace");
        c.retain(|s| s != 7);
        assert!(c.is_empty());
        c.insert("z", "z", 10, None, 0);
        c.clear();
        assert_eq!((c.len(), c.stats().bytes), (0, 0));
        let s = c.stats();
        assert_eq!((s.hits, s.misses), (1, 2), "{s:?}");
    }

    #[test]
    fn json_sizes_grow_with_the_text() {
        let small = json_bytes(&json!({ "a": "x" }));
        let big = json_bytes(&json!({ "a": "x".repeat(1000) }));
        assert_eq!(big - small, 999);
        assert!(json_bytes(&json!([1, 2, 3])) > json_bytes(&json!([1])));
    }
}
