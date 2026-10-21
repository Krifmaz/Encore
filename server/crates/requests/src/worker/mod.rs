//! The request worker, stage by stage: `queue` gives every active request a task, `pick` decides where
//! it comes from, `fetch` runs the sources (streamrip, Soulseek, Prowlarr → the download clients,
//! YouTube), `verify` holds what a download is checked against, and `import` tags and imports the files
//! and finishes the request. [`storage`] keeps a download whose import hits a storage error and runs its
//! import again later (the `waiting` status), with the health check that pauses imports. [`rejected`]
//! keeps a download the audio check turned away, for Keep anyway and Download anyway. [`downloads`] probes
//! the folders downloads are saved in every minute; a request whose source saves in one that can't be
//! written waits for it (the `waiting` status again) instead of failing. [`now`] is Download now: what a
//! waiting or queued request offers, and doing it without breaking a source's limit or a ban
//! (the lines are [`crate::line`]). [`steps`] writes what a long step is at into its request's message.
//! [`stall`] finds an import that hasn't moved for 15 minutes (Import again stops it and imports the kept
//! files at once) and is the watchdog that starts a search or an import whose task is gone again.
pub mod downloads;
mod fetch;
mod import;
pub mod now;
mod pick;
mod queue;
pub mod rejected;
pub mod stall;
mod steps;
pub mod storage;
mod verify;

pub use pick::{ALBUM_RETRY_SECS, ALBUM_TRIES, ALBUM_WAIT};
pub use queue::{cancel, cancel_holding, hold_idle, recover, run, step, stop_holding, take_turn, Hold};
pub use stall::watchdog;
use queue::on_error;

use crate::sources::{slskd, streamrip, torznab, ytdlp};
use crate::{notify, Ctx};
use encore_db::{now, p, Row, RowExt};
use encore_http::mb;
use encore_library::tagger::{self, Check, Rejected};
use fetch::*;
use import::*;
// who added a request's songs (tracks.added_by / added_via / requested_at), for the server's tests too
pub use import::{adder_of, record_origin};
use pick::*;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use verify::*;

/// The states of a request the worker is busy with. `waiting`: the request waits for something outside it
/// (`detail.wait.reason`: "soulseek" for the Soulseek session, "storage_import" for its import after a
/// storage error, its files kept, "storage_download" for the staging folder, "waiting_disk" for a downloads
/// folder the probe found unwritable); the worker looks at it again when its turn comes
/// (`detail.wait.next_try_at`, or the probe passing) and never starts it as a new download.
pub const ACTIVE: [&str; 6] = ["queued", "searching", "downloading", "monitoring", "importing", "waiting"];

pub use steps::Steps;
pub use storage::Health;

static RIP_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"qobuz\.com|tidal\.com|deezer\.com").unwrap());

/// Running request tasks, so a cancel can stop them.
#[derive(Clone, Default)]
pub struct Tasks(Arc<Mutex<HashMap<i64, tokio::task::AbortHandle>>>);

impl Tasks {
    /// Whether the request's place in the queue is taken now (a step running, or held by [`Hold`]): the
    /// queue starts no step for it.
    pub fn taken(&self, id: i64) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).get(&id).is_some_and(|h| !h.is_finished())
    }
}

/// The request was cancelled/removed while we worked on it.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A request as the worker sees it: its row and its parsed `detail`.
pub struct Req {
    pub id: i64,
    pub row: Row,
    pub d: Value,
}

impl Req {
    fn s(&self, k: &str) -> String {
        self.row.s(k)
    }
    fn opt(&self, k: &str) -> Option<String> {
        Some(self.row.s(k)).filter(|x| !x.is_empty())
    }
    fn album_title(&self) -> String {
        self.opt("album").unwrap_or_else(|| self.s("title"))
    }
    fn length(&self) -> Option<f64> {
        self.d["length"].as_f64().filter(|l| *l > 0.0)
    }
    fn label(&self) -> String {
        match self.s("kind").as_str() {
            "album" => format!("{} – {}", self.s("artist"), self.album_title()),
            "artist" => self.s("artist"),
            "url" => self.opt("title").unwrap_or_else(|| self.s("url")),
            _ => format!("{} – {}", self.s("artist"), self.s("title")),
        }
    }
    fn hint(&self) -> Option<String> {
        self.opt("cover_url")
    }
    /// `stereo` (every request before 0.35) or `surround`: a request for the Surround library.
    fn surround(&self) -> bool {
        self.s("class") == "surround"
    }
}

pub fn get(st: &impl Ctx, id: i64) -> Option<Req> {
    let row = st.db().one("SELECT * FROM requests WHERE id=?", p![id]).ok()??;
    let d = serde_json::from_str(&row.s("detail")).unwrap_or(json!({}));
    let d = if d.is_object() { d } else { json!({}) };
    Some(Req { id, row, d })
}

/// Update columns (detail is always written along).
fn upd(st: &impl Ctx, r: &mut Req, fields: &[(&str, Value)]) {
    let mut sets = vec![];
    let mut params = vec![];
    for (k, v) in fields {
        sets.push(format!("{k}=?"));
        params.push(v.clone());
        r.row.insert(k.to_string(), v.clone());
    }
    sets.push("detail=?".into());
    params.push(json!(r.d.to_string()));
    sets.push("updated_at=?".into());
    params.push(json!(now()));
    params.push(json!(r.id));
    let _ = st.db().execute(&format!("UPDATE requests SET {} WHERE id=?", sets.join(", ")), params);
    if fields.iter().any(|(k, _)| *k == "status" || *k == "progress") {
        crate::events::emit(st, r.id); // live labels on artist and album pages
    }
}

fn check(st: &impl Ctx, id: i64) -> anyhow::Result<()> {
    match get(st, id) {
        Some(r) if ACTIVE.contains(&r.s("status").as_str()) => Ok(()),
        _ => Err(Cancelled.into()),
    }
}

fn staging(st: &impl Ctx, r: &Req) -> PathBuf {
    PathBuf::from(st.cfg().str("staging_path")).join(format!("request-{}", r.id))
}

fn rm(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// What happened when trying a source.
enum Out {
    Done,
    Wait,
    Next(String),
}

fn log_push(r: &mut Req, line: String) {
    if !r.d["log"].is_array() {
        r.d["log"] = json!([]);
    }
    r.d["log"].as_array_mut().unwrap().push(json!(line));
}

fn with_note(msg: &str, note: &str) -> String {
    if note.is_empty() { msg.into() } else { format!("{msg} – {note}") }
}

/// Run `f` with what it is at written into request `id`'s message while the row's status is `status`:
/// "{base} – {step}" for each step it names and each wait it sits in ("Importing – checking the audio",
/// "Importing – waiting for MusicBrainz (about 2 minutes, 120 lookups ahead)": encore_http::progress), and
/// `before` – the message it had – when it is at no named step. The words are written from one place, off
/// the async threads ([`steps`]); the message alone changing tells no open page (the request events carry no
/// message). Once `f` is done the message is `before` again, unless something else wrote one meanwhile (the
/// request finished, say). `f`'s MusicBrainz calls keep one place in the line ([`mb::one_place`]), so a step
/// that makes several finishes before the ones that started after it.
async fn with_steps<F: std::future::Future>(st: &impl Ctx, id: i64, status: &str, base: &str, before: &str, f: F) -> F::Output {
    let scope = st.requests().steps.begin(st.db(), id, status, before);
    let (say, base_s, before_s) = (scope.sayer(), base.to_string(), before.to_string());
    // (`f` boxed: an import's future is large, and a step's runs on a worker thread's stack)
    let out = encore_http::progress::with_sink(move |step: &str| {
        say(if step.is_empty() { before_s.clone() } else { format!("{base_s} – {step}") });
    }, mb::one_place(Box::pin(f))).await;
    scope.end().await;
    out
}
