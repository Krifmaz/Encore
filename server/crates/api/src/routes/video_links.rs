//! Download a video from a link straight to your device (Downloader → Save video to this device): the server
//! fetches the MP4 with yt-dlp into a folder of its own, hands it to the device that asked, and deletes it.
//! The video never goes into the library and is never kept: a fetched copy is removed the moment it was sent
//! whole, a failed or cancelled one at once, and one nobody fetched after [`KEEP_READY`].
//!
//! - `POST /api/video-links` `{url}` → the job (below): starts downloading (at most [`SLOTS`] at once on the
//!   server, at most [`PER_USER`] unfetched per account).
//! - `GET /api/video-links/{id}` → `{id, state, progress, title, name, size, error}`; `state` is `queued`,
//!   `downloading`, `ready` or `failed`.
//! - `GET /api/video-links/{id}/file` → the MP4 as an attachment; the server's copy is deleted once the
//!   whole file was sent (a broken-off transfer may ask again until it expires).
//! - `DELETE /api/video-links/{id}` → cancel (stops yt-dlp) and delete.
//!
//! Each job belongs to the account that started it: anyone else gets a 404. Accounts need the
//! `video_download` permission (on by default) and the yt-dlp plugin must be switched on.
use crate::auth::User;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_db::RowExt;
use encore_requests::sources::ytdlp;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_util::io::ReaderStream;

/// The permission that lets an account download videos to its device (admins always may).
pub const PERM: &str = "video_download";
/// Videos downloading on the server at once; later ones wait their turn (`queued`).
pub const SLOTS: usize = 2;
/// Jobs one account may have that were not fetched or cancelled yet.
pub const PER_USER: usize = 3;
/// How long a downloaded video waits to be fetched (or a failed job's error to be read) before it is deleted.
pub const KEEP_READY: Duration = Duration::from_secs(15 * 60);
/// The folder the downloads go in, inside the staging folder (else the config folder).
const FOLDER: &str = "video-links";

/// What a download step is given: the link, the folder to download into, and where progress (0–100) goes.
/// The real one is yt-dlp ([`ytdlp::download_video`]); tests put a stand-in in its place.
pub(crate) type Fetch = Arc<dyn Fn(AppState, String, PathBuf, Progress) -> futures_util::future::BoxFuture<'static, anyhow::Result<ytdlp::Video>> + Send + Sync>;
/// Where a download's progress goes (0–100).
pub(crate) type Progress = Arc<dyn Fn(f64) + Send + Sync>;

/// Where a job stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// waiting for a free slot
    Queued,
    /// yt-dlp is running
    Downloading,
    /// the MP4 is there, waiting to be fetched
    Ready,
    /// yt-dlp failed (its folder is already gone; the error stays readable until it expires)
    Failed,
}

impl Phase {
    /// The word the API answers with.
    fn as_str(self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Downloading => "downloading",
            Phase::Ready => "ready",
            Phase::Failed => "failed",
        }
    }
}

/// One video download.
struct Job {
    /// the account that started it (the only one that may see, fetch or cancel it)
    user_id: i64,
    /// the job's own folder (removed with the job)
    dir: PathBuf,
    /// where it stands
    phase: Phase,
    /// 0–100 while downloading
    progress: f64,
    /// the video's title, once yt-dlp said
    title: Option<String>,
    /// the MP4, once ready
    file: Option<PathBuf>,
    /// the MP4's size in bytes, once ready
    size: u64,
    /// why it failed
    error: Option<String>,
    /// the download task (aborted on cancel, which stops yt-dlp: its process is killed on drop)
    task: Option<tokio::task::AbortHandle>,
}

impl Job {
    /// The job as the API shows it.
    fn json(&self, id: &str) -> Value {
        json!({ "id": id, "state": self.phase.as_str(), "progress": self.progress, "title": self.title,
                "name": self.file.as_ref().map(|_| download_name(self.title.as_deref())), "size": self.size, "error": self.error })
    }
}

/// The video downloads in progress or waiting to be fetched (`AppState::web`). Clones share them.
#[derive(Clone)]
pub struct VideoLinks {
    /// every job by its id (a random, unguessable token)
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    /// one permit per download running ([`SLOTS`])
    slots: Arc<tokio::sync::Semaphore>,
    /// the download step: None is yt-dlp; tests put a stand-in here ([`VideoLinks::fake`])
    fetch: Arc<Mutex<Option<Fetch>>>,
}

impl Default for VideoLinks {
    fn default() -> Self {
        VideoLinks { jobs: Default::default(), slots: Arc::new(tokio::sync::Semaphore::new(SLOTS)), fetch: Default::default() }
    }
}

impl VideoLinks {
    /// The jobs, locked (a panic elsewhere while holding them doesn't lock everyone out).
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Let `f` download instead of yt-dlp from now on. Tests only.
    #[cfg(test)]
    pub(crate) fn fake(&self, f: Fetch) {
        *self.fetch.lock().unwrap() = Some(f);
    }

    /// Remove job `id` and its folder (nothing when it is already gone). A running download is stopped first.
    fn remove(&self, id: &str) {
        let job = self.lock().remove(id);
        if let Some(j) = job {
            if let Some(t) = j.task {
                t.abort();
            }
            let _ = std::fs::remove_dir_all(&j.dir);
        }
    }

    /// Remove job `id` after [`KEEP_READY`] unless it is gone by then (fetched or cancelled).
    fn expire_later(&self, id: String) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(KEEP_READY).await;
            me.remove(&id);
        });
    }
}

/// The routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/video-links", post(start))
        .route("/api/video-links/{id}", get(status).delete(cancel))
        .route("/api/video-links/{id}/file", get(file))
}

/// The folder every job's folder goes in: the staging folder's `video-links` when the staging folder is set
/// (the downloads' disk), else the config folder's.
pub fn root(st: &AppState) -> PathBuf {
    let staging = PathBuf::from(st.cfg.str("staging_path"));
    if staging.is_absolute() { staging.join(FOLDER) } else { st.data_dir().join(FOLDER) }
}

/// Remove whatever a server that stopped mid-download left in [`root`] (called once at start).
pub fn clear_leftovers(st: &AppState) {
    let _ = std::fs::remove_dir_all(root(st));
}

/// The file name a device saves a video as: its title made safe for every file system, `.mp4`.
pub fn download_name(title: Option<&str>) -> String {
    let t: String = title.unwrap_or("").chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let t = t.trim().trim_end_matches('.').trim();
    // long titles cut on a character boundary, never in the middle of one
    let t: String = t.chars().take(150).collect();
    format!("{}.mp4", if t.is_empty() { "video" } else { t.trim_end() })
}

/// `POST /api/video-links` body.
#[derive(Deserialize)]
struct StartBody {
    /// the link to the video
    url: String,
}

/// POST /api/video-links: start downloading a video for this device.
async fn start(State(st): State<AppState>, User(u): User, Json(b): Json<StartBody>) -> ApiResult<Json<Value>> {
    let url = b.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(err(400, "Paste a full link (https://…)"));
    }
    if !crate::auth::can(&st, &u, PERM) {
        return Err(err(403, "Ask an admin to let you download videos"));
    }
    let links = st.web.video_links.clone();
    let fetch = links.fetch.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if fetch.is_none() && !ytdlp::plugin_on(&st) {
        return Err(err(400, "Video downloads need the yt-dlp plugin – install it under Settings → Plugins"));
    }
    let user_id = u.i("id");
    let id = encore_core::ids::random_token(16);
    let dir = root(&st).join(&id);
    {
        let mut jobs = links.lock();
        if jobs.values().filter(|j| j.user_id == user_id && j.phase != Phase::Failed).count() >= PER_USER {
            return Err(err(429, format!("You already have {PER_USER} videos on the way – save or cancel one first")));
        }
        jobs.insert(id.clone(), Job { user_id, dir: dir.clone(), phase: Phase::Queued, progress: 0.0, title: None, file: None, size: 0, error: None, task: None });
    }
    let fetch: Fetch = fetch.unwrap_or_else(|| Arc::new(|st: AppState, url: String, dir: PathBuf, progress: Progress| {
        Box::pin(async move { ytdlp::download_video(&st, &url, &dir, Some(&*progress)).await })
    }));
    st.db.log("info", &format!("{} is downloading a video to their device: {url}", u.s("username")));
    let (l, jid, st2) = (links.clone(), id.clone(), st.clone());
    let task = tokio::spawn(async move {
        // waits here while SLOTS others download; a closed semaphore never happens (it lives as long as `l`)
        let Ok(_permit) = l.slots.clone().acquire_owned().await else { return };
        if let Some(j) = l.lock().get_mut(&jid) {
            j.phase = Phase::Downloading;
        }
        let progress: Progress = {
            let (l, jid) = (l.clone(), jid.clone());
            Arc::new(move |p: f64| {
                if let Some(j) = l.lock().get_mut(&jid) {
                    j.progress = p.clamp(0.0, 100.0);
                }
            })
        };
        let res = fetch(st2, url, dir.clone(), progress).await;
        let mut jobs = l.lock();
        let Some(j) = jobs.get_mut(&jid) else {
            // cancelled meanwhile: nothing may stay behind
            drop(jobs);
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        match res {
            Ok(v) => {
                j.size = std::fs::metadata(&v.file).map(|m| m.len()).unwrap_or(0);
                j.phase = Phase::Ready;
                j.progress = 100.0;
                j.title = v.title.or_else(|| v.file.file_stem().map(|s| s.to_string_lossy().into_owned()));
                j.file = Some(v.file);
            }
            Err(e) => {
                j.phase = Phase::Failed;
                j.error = Some(e.to_string().chars().take(300).collect());
                // nothing to hand over: the folder goes now, the error stays readable a while
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        j.task = None;
        drop(jobs);
        l.expire_later(jid);
    });
    let mut jobs = links.lock();
    let Some(j) = jobs.get_mut(&id) else { return Err(err(500, "The download vanished")) };
    // a task that already finished keeps no handle (aborting it would do nothing anyway)
    if matches!(j.phase, Phase::Queued | Phase::Downloading) {
        j.task = Some(task.abort_handle());
    }
    Ok(Json(j.json(&id)))
}

/// The caller's job `id` as the API shows it; 404 for anyone else's (or none).
fn owned<T>(st: &AppState, u: &encore_db::Row, id: &str, f: impl FnOnce(&Job) -> T) -> ApiResult<T> {
    let jobs = st.web.video_links.lock();
    match jobs.get(id) {
        Some(j) if j.user_id == u.i("id") => Ok(f(j)),
        _ => Err(err(404, "No such video download (it may have expired)")),
    }
}

/// GET /api/video-links/{id}: where a download stands.
async fn status(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    owned(&st, &u, &id, |j| Json(j.json(&id)))
}

/// DELETE /api/video-links/{id}: stop a download, or drop one that was not fetched, and delete its file.
async fn cancel(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    owned(&st, &u, &id, |_| ())?;
    st.web.video_links.remove(&id);
    Ok(Json(json!({ "ok": true })))
}

/// GET /api/video-links/{id}/file: the MP4 as an attachment. The server's copy is deleted once it was sent
/// whole ([`Handover`]).
async fn file(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Response> {
    let (path, title, size) = owned(&st, &u, &id, |j| (j.file.clone().filter(|_| j.phase == Phase::Ready), j.title.clone(), j.size))?;
    let path = path.ok_or_else(|| err(409, "The video isn't ready yet"))?;
    let f = tokio::fs::File::open(&path).await.map_err(|_| err(404, "The video is gone (it may have expired)"))?;
    let body = Body::from_stream(Handover { inner: ReaderStream::new(f), sent: 0, size, id, links: st.web.video_links.clone() });
    let mut resp = body.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(header::CONTENT_DISPOSITION, disposition(&download_name(title.as_deref())));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(resp)
}

/// `attachment; filename="…"; filename*=UTF-8''…`: an ASCII stand-in for old clients and the real name.
fn disposition(name: &str) -> HeaderValue {
    let ascii: String = name.chars().map(|c| if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' { c } else { '_' }).collect();
    HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", urlencoding::encode(name)))
        .unwrap_or(HeaderValue::from_static("attachment"))
}

/// The MP4 on its way to the device: the file's bytes, counted. When the body is dropped after every byte
/// went out, the job and its folder are removed; a transfer broken off before that leaves it for another try
/// until it expires.
struct Handover {
    /// the file, chunk by chunk
    inner: ReaderStream<tokio::fs::File>,
    /// bytes handed on so far
    sent: u64,
    /// the file's size
    size: u64,
    /// the job
    id: String,
    /// the jobs (to remove this one)
    links: VideoLinks,
}

impl Stream for Handover {
    type Item = std::io::Result<Bytes>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = Pin::new(&mut self.inner).poll_next(cx);
        if let Poll::Ready(Some(Ok(b))) = &next {
            self.sent += b.len() as u64;
        }
        next
    }
}

impl Drop for Handover {
    fn drop(&mut self) {
        if self.sent >= self.size {
            self.links.remove(&self.id);
        }
    }
}

#[cfg(test)]
#[path = "video_links_tests.rs"]
mod tests;
