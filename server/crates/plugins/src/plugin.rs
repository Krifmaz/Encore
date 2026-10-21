//! The `Plugin` trait every plugin answers through: the builtin ones in-process (encore-requests, encore-system,
//! encore-library, encore-playback register theirs with the registry) and the uploaded ones as a
//! [`crate::process::ProcessPlugin`] speaking protocol 1. Installing, updating, enabling and removing are the
//! registry's ([`crate::registry::Registry`]): it reads the manifest, runs the installer and keeps the plugins
//! table, then tells the plugin through the hooks here, so a builtin can restart its program.
use crate::manifest::Manifest;
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The error codes of protocol 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    Auth,
    NotFound,
    Network,
    RateLimited,
    Unsupported,
    Cancelled,
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Auth => "auth",
            Code::NotFound => "not_found",
            Code::Network => "network",
            Code::RateLimited => "rate_limited",
            Code::Unsupported => "unsupported",
            Code::Cancelled => "cancelled",
            Code::Internal => "internal",
        }
    }

    /// A code from a plugin's answer; an unknown one counts as `internal`.
    pub fn parse(s: &str) -> Code {
        match s {
            "auth" => Code::Auth,
            "not_found" => Code::NotFound,
            "network" => Code::Network,
            "rate_limited" => Code::RateLimited,
            "unsupported" => Code::Unsupported,
            "cancelled" => Code::Cancelled,
            _ => Code::Internal,
        }
    }
}

/// A command that did not succeed: the plugin's error (shown as it is), or Encore's about the plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: Code,
    pub message: String,
    /// rate_limited: seconds until the next command may go
    pub retry_after: Option<f64>,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure { code, message: message.into(), retry_after: None }
    }

    pub fn internal(message: impl Into<String>) -> Failure {
        Failure::new(Code::Internal, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Failure {
        Failure::new(Code::Unsupported, message)
    }

    pub fn cancelled() -> Failure {
        Failure::new(Code::Cancelled, "cancelled")
    }

    /// `{ok: false, error, code, retry_after}`.
    pub fn json(&self) -> Value {
        let mut v = json!({ "ok": false, "error": self.message, "code": self.code.as_str() });
        if let Some(r) = self.retry_after {
            v["retry_after"] = json!(r);
        }
        v
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Failure {
        Failure::internal(encore_core::fsx::explain(&e))
    }
}

/// A plugin's state line (`status`).
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub message: String,
    /// false: the card and the picker say "sign in"
    pub signed_in: Option<bool>,
}

/// One file a download delivered (the SDK's `file_entry()`), checked: inside the request's folder, a regular
/// audio file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// absolute, links resolved
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<i64>,
    pub disc: Option<i64>,
    pub duration: Option<f64>,
    pub format: Option<String>,
    pub size: Option<u64>,
    pub isrc: Option<String>,
}

/// What a download delivered.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Downloaded {
    pub files: Vec<FileEntry>,
    pub cover_url: Option<String>,
    pub source_url: Option<String>,
    pub message: Option<String>,
}

/// Progress of a download: (percent 0–100, what is happening).
pub type Progress = Arc<dyn Fn(f64, Option<String>) + Send + Sync>;

/// What a call carries: the plugin's settings (secrets included: a process plugin gets them on stdin only), the
/// progress bar to drive and the switch that stops it.
#[derive(Clone, Default)]
pub struct Call {
    pub settings: serde_json::Map<String, Value>,
    pub cancel: CancellationToken,
    pub progress: Option<Progress>,
}

impl Call {
    pub fn with_settings(settings: serde_json::Map<String, Value>) -> Call {
        Call { settings, ..Call::default() }
    }
}

/// The newest version a check found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub version: String,
    /// the release's tag (GitHub), or the version (PyPI)
    pub tag: String,
}

/// A plugin. Every method but `manifest` has a default, so a plugin answers only what it does.
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &Manifest;

    /// A line about its state (the card, the picker's source line). Quick, off the network when possible.
    fn status<'a>(&'a self, _call: &'a Call) -> BoxFuture<'a, Result<Status, Failure>> {
        Box::pin(async { Ok(Status { message: "Ready".into(), signed_in: None }) })
    }

    /// The card's Test button: a line about what was checked.
    fn test<'a>(&'a self, _call: &'a Call) -> BoxFuture<'a, Result<String, Failure>> {
        Box::pin(async { Err(Failure::unsupported("This plugin has no test")) })
    }

    /// A source's search: the request's fields (kind, artist, title, album, query, length, track_count,
    /// lossless_only, limit, isrc, url) → results in the shape of the SDK's `result()`, best first.
    fn search<'a>(&'a self, _call: &'a Call, _request: &'a Value) -> BoxFuture<'a, Result<Vec<Value>, Failure>> {
        Box::pin(async { Err(Failure::unsupported("This plugin does not search")) })
    }

    /// A source's download of a result (its `key` and `payload` in `request`) into `folder`, an empty folder
    /// Encore made for it.
    fn download<'a>(&'a self, _call: &'a Call, _request: &'a Value, _folder: &'a Path) -> BoxFuture<'a, Result<Downloaded, Failure>> {
        Box::pin(async { Err(Failure::unsupported("This plugin does not download")) })
    }

    /// The newest version, when the plugin finds it itself; None: the registry reads `[update]`.
    fn check_update<'a>(&'a self, _call: &'a Call) -> BoxFuture<'a, Option<Result<Found, Failure>>> {
        Box::pin(async { None })
    }

    /// After the registry installed or updated it (`version`: the program's version, when it installs one): a
    /// builtin restarts its program on the new files. The protocol's own `update` command runs here too.
    fn installed<'a>(&'a self, _call: &'a Call, _version: Option<&'a str>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async { Ok(()) })
    }

    /// Before the registry removes its folder: stop what runs.
    fn removing<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Switched on or off on its card.
    fn enable<'a>(&'a self, _on: bool) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async { Ok(()) })
    }
}
