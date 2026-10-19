//! Live request labels. Every status change of a request (requested → searching → downloading →
//! importing → in library, or not found / failed) is pushed to the open event streams of its owner and
//! of the admins (`GET /api/requests/events`), so the badges on artist and album pages update in place.
//!
//! Each event carries `keys`: the ways a page can know the release or song – `rg:<release group>`,
//! `rec:<recording>`, `ar:<artist>`, `ext:deezer:<id>` / `ext:apple:<id>` …, and `a:`/`t:` + the
//! normalized artist and title. A page marks its badges with the same keys (`data-req-key`).
use crate::Ctx;
use encore_core::text::norm;
use encore_db::{p, Db, Row, RowExt};
use serde_json::{json, Value};
use tokio::sync::broadcast;

/// The hub the open event streams listen to: (owner user id, event). The server keeps one; clones share it.
#[derive(Clone)]
pub struct Events(broadcast::Sender<(i64, Value)>);

impl Default for Events {
    fn default() -> Self {
        Events(broadcast::channel(512).0)
    }
}

impl Events {
    /// A new listener (an open `/api/requests/events` stream).
    pub fn subscribe(&self) -> broadcast::Receiver<(i64, Value)> {
        self.0.subscribe()
    }

    /// Tell the open streams about request `id`'s current status (read from `db`).
    pub fn emit(&self, db: &Db, id: i64) {
        if self.0.receiver_count() == 0 {
            return;
        }
        if let Ok(Some(r)) = db.one("SELECT id, user_id, kind, mbid, artist, title, album, status, message, progress, detail, parent_id FROM requests WHERE id=?", p![id]) {
            let _ = self.0.send((r.i("user_id"), event_for(&r)));
        }
    }
}

/// The words a page shows for a request status.
pub fn label(status: &str, message: &str) -> &'static str {
    match status {
        "pending" => "Awaiting approval",
        "queued" => "Requested",
        "searching" => "Searching",
        "downloading" | "monitoring" => "Downloading",
        "importing" => "Importing",
        "completed" => "In library",
        "failed" => {
            // no source had it ("All sources exhausted (…)") – unless what went wrong was the import; a
            // configuration error such as "Downloaded files not found under …" is a failure
            let m = message.to_lowercase();
            let exhausted = m.starts_with("all sources exhausted") && !m.contains("import");
            if exhausted || m.starts_with("no results") || m.starts_with("nothing found") { "Not found" } else { "Failed" }
        }
        "denied" => "Denied",
        "cancelled" => "Cancelled",
        _ => "Requested",
    }
}

/// Every key a page may know this request by.
pub fn keys(kind: &str, mbid: &str, artist: &str, title: &str, detail: &Value) -> Vec<String> {
    let mut k = vec![];
    if !mbid.is_empty() {
        match kind {
            "album" => k.push(format!("rg:{mbid}")),
            "track" => k.push(format!("rec:{mbid}")),
            "artist" => k.push(format!("ar:{mbid}")),
            _ => {}
        }
    }
    if let Some(r) = detail["recording"].as_str().filter(|r| !r.is_empty() && kind == "track") {
        k.push(format!("rec:{r}"));
    }
    if let Some(e) = detail["ext"].as_object() {
        for (name, id) in e {
            if name == "upc" {
                continue;
            }
            let id = id.as_str().map(String::from).unwrap_or(id.to_string());
            if !id.is_empty() && id != "null" {
                k.push(format!("ext:{name}:{id}"));
            }
        }
    }
    let (a, t) = (norm(artist), norm(title));
    if !t.is_empty() {
        match kind {
            "album" => k.push(format!("a:{a}|{t}")),
            "track" => k.push(format!("t:{a}|{t}")),
            _ => {}
        }
    }
    k
}

/// The event for one request row.
pub fn event_for(r: &Row) -> Value {
    let detail: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
    let kind = r.s("kind");
    let title = if kind == "album" { Some(r.s("album")).filter(|a| !a.is_empty()).unwrap_or(r.s("title")) } else { r.s("title") };
    let ks = keys(&kind, &r.s("mbid"), &r.s("artist"), &title, &detail);
    let ext_key = ks.iter().find(|k| k.starts_with("ext:")).cloned();
    let status = r.s("status");
    // an artist request is done once its releases are queued: those carry their own labels
    let lbl = if kind == "artist" && status == "completed" { "Discography requested" } else { label(&status, &r.s("message")) };
    json!({ "type": "request", "request_id": r.i("id"), "kind": kind, "mbid": Some(r.s("mbid")).filter(|m| !m.is_empty()),
            "ext_key": ext_key, "keys": ks, "artist": r.s("artist"), "title": title, "parent_id": r.get("parent_id").cloned().unwrap_or(Value::Null),
            "status": status, "label": lbl, "progress": r.f("progress") })
}

/// Tell the open streams about the request's current status.
pub fn emit(st: &impl Ctx, id: i64) {
    st.requests().events.emit(st.db(), id);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_and_keys() {
        assert_eq!(label("queued", ""), "Requested");
        assert_eq!(label("monitoring", ""), "Downloading");
        assert_eq!(label("completed", ""), "In library");
        assert_eq!(label("failed", "All sources exhausted (slskd: no results)"), "Not found");
        assert_eq!(label("failed", "Import failed: disk full"), "Failed");
        assert_eq!(label("failed", "Downloaded files not found under /downloads – check the shared volume mapping"), "Failed");
        assert_eq!(label("failed", "All sources exhausted (slskd: import failed: no audio)"), "Failed");
        let k = keys("album", "", "Travis Scott", "Rodeo (Deluxe)", &json!({ "ext": { "deezer": 123, "apple": "456", "upc": "0001" } }));
        assert!(k.contains(&"ext:deezer:123".to_string()), "{k:?}");
        assert!(k.contains(&"ext:apple:456".to_string()), "{k:?}");
        assert!(k.contains(&"a:travisscott|rodeodeluxe".to_string()), "{k:?}");
        assert!(!k.iter().any(|x| x.contains("upc")));
        assert_eq!(keys("album", "rg1", "A", "B", &json!({}))[0], "rg:rg1");
        assert_eq!(keys("track", "rec1", "Beyoncé", "Halo", &json!({}))[1], "t:beyoncé|halo");
    }
}
