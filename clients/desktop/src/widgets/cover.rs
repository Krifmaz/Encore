//! Cover art: an in-memory LRU of `iced::widget::image::Handle`s keyed by URL. Pages ask
//! `ctx.cover(...)` while drawing; a miss is recorded as a want, the app fetches it (with the
//! session cookie) and the picture shows on the next frame. Sizes are the web's (`?s=400`),
//! `cover_v` busts the cache the same way.

use crate::api::Api;
use iced::widget::image;
use iced::Task;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// How many pictures stay in memory.
const CAP: usize = 400;

#[derive(Default)]
struct Inner {
    map: HashMap<String, Option<image::Handle>>,
    order: VecDeque<String>,
    wants: Vec<String>,
    inflight: HashSet<String>,
}

/// The cache; `&self` everywhere so views can record wants.
#[derive(Default)]
pub struct Covers {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Covers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        write!(f, "Covers({} loaded, {} wanted)", g.map.len(), g.wants.len())
    }
}

impl Covers {
    pub fn new() -> Self {
        Self::default()
    }

    /// The picture for `url` if loaded; an unknown URL is queued for fetching.
    pub fn get(&self, url: &str) -> Option<image::Handle> {
        if url.is_empty() {
            return None;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.map.get(url) {
            Some(h) => h.clone(),
            None => {
                if !g.inflight.contains(url) && !g.wants.iter().any(|w| w == url) {
                    g.wants.push(url.to_string());
                }
                None
            }
        }
    }

    /// The URLs views asked for since the last call; they are marked in flight.
    pub fn take_wants(&self) -> Vec<String> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let wants = std::mem::take(&mut g.wants);
        for w in &wants {
            g.inflight.insert(w.clone());
        }
        wants
    }

    /// Store a fetched picture (`None` = failed, remembered so it is not asked again).
    pub fn insert(&self, url: String, bytes: Option<Vec<u8>>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.inflight.remove(&url);
        g.map.insert(url.clone(), bytes.map(image::Handle::from_bytes));
        g.order.push_back(url);
        while g.order.len() > CAP {
            if let Some(old) = g.order.pop_front() {
                g.map.remove(&old);
            }
        }
    }

    /// Forget one picture (after the artwork changed).
    pub fn forget(&self, url: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map.remove(url);
    }

    /// Forget everything (sign out, server change).
    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *g = Inner::default();
    }
}

/// A library album's cover URL, as web-rs `ui::cover_url`: `/api/library/albums/{id}/cover?s=400&v=…`.
/// `cover_v` is the album's or song's `cover_v` field (Null when it has none).
pub fn album_url(base: &str, album_id: &str, cover_v: &Value, size: u32) -> String {
    let mut q = Vec::new();
    if size > 0 {
        q.push(format!("s={size}"));
    }
    match cover_v {
        Value::Number(n) => q.push(format!("v={n}")),
        Value::String(s) if !s.is_empty() => q.push(format!("v={s}")),
        _ => {}
    }
    format!(
        "{base}/api/library/albums/{album_id}/cover{}",
        if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) }
    )
}

/// A catalog artist's picture (`/api/discover/artist/{id}/image`), or any full URL as is.
pub fn absolute(base: &str, url_or_path: &str) -> String {
    if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        url_or_path.to_string()
    } else {
        format!("{base}{url_or_path}")
    }
}

/// Fetch one picture; the result goes back as `(url, bytes)`.
pub fn load(api: Arc<Api>, url: String) -> Task<(String, Option<Vec<u8>>)> {
    Task::perform(
        async move {
            let bytes = api.get_bytes(&url).await.ok().filter(|b| !b.is_empty());
            (url, bytes)
        },
        |r| r,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_match_the_web() {
        assert_eq!(
            album_url("http://s", "abc", &Value::Null, 400),
            "http://s/api/library/albums/abc/cover?s=400"
        );
        assert_eq!(
            album_url("http://s", "abc", &serde_json::json!(7), 0),
            "http://s/api/library/albums/abc/cover?v=7"
        );
        assert_eq!(absolute("http://s", "/api/x"), "http://s/api/x");
        assert_eq!(absolute("http://s", "https://y/z"), "https://y/z");
    }

    #[test]
    fn misses_become_wants_once() {
        let c = Covers::new();
        assert!(c.get("u1").is_none());
        assert!(c.get("u1").is_none());
        assert_eq!(c.take_wants(), vec!["u1".to_string()]);
        assert!(c.take_wants().is_empty(), "in flight, not asked again");
        c.insert("u1".into(), None);
        assert!(c.get("u1").is_none());
        assert!(c.take_wants().is_empty(), "a failed picture is not retried");
    }
}
