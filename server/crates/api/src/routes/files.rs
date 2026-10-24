//! /api/files: the admin file manager (release 0.33) over the folders Encore may touch – browse, search by name, upload, make
//! folder, rename, move, Trash (restore, delete forever, empty), tags and import from a folder. Admin only,
//! CSRF on every write (the `Admin` extractor). The work is encore-files' (confinement, no writes through
//! links, the library's rows following, the activity log); the JSON is docs/contracts/client-shapes-0.31-0.33.md
//! §2.4 and Part 4. File work runs on the blocking pool.
use crate::auth::Admin;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_files::import::{self, Mode};
use encore_files::list::{self, Query as ListQuery};
use encore_files::roots::{self, LibraryDirs, Root};
use encore_files::rows::Change;
use encore_files::upload::{self, Begin, Conflict, Outcome};
use encore_files::{ops, search, tags, trash, Target, Who};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The most an upload request may carry: the backups upload's limit (routes::system).
pub const UPLOAD_LIMIT: usize = 4 << 30;

/// The file manager routes (admins).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/files/roots", get(get_roots))
        .route("/api/files/list", get(list_dir))
        .route("/api/files/search", get(search_names))
        .route("/api/files/upload", post(upload_files).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/files/mkdir", post(mkdir))
        .route("/api/files/rename", post(rename))
        .route("/api/files/move", post(move_items))
        .route("/api/files/trash", get(trash_items).post(to_trash))
        .route("/api/files/trash/{id}", delete(delete_forever))
        .route("/api/files/restore", post(restore))
        .route("/api/files/empty-trash", post(empty_trash))
        .route("/api/files/tags", get(get_tags).put(put_tags))
        .route("/api/files/import/preview", post(import_preview))
        .route("/api/files/import/match", post(import_match))
        .route("/api/files/import/apply", post(import_apply))
}

/// The roots as they are now (the settings and Soulseek's folder in use).
fn roots_of(st: &AppState) -> Vec<Root> {
    roots::all(&st.cfg, st.data_dir(), &encore_requests::slskd_managed::download_dir(st))
}

/// `root` + `path` of a request, checked (the root must be one of them, the path confined to it).
fn target(roots: &[Root], root: &str, path: &str) -> ApiResult<Target> {
    Target::new(roots::find(roots, root)?, path)
}

/// File work on the blocking pool.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> ApiResult<T> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| err(500, format!("The file operation stopped: {e}")))?
}

/// GET /api/files/roots: the folders the file manager may open. Trash's also says whether deletes keep files
/// there (`keep`, Settings → Library → Deleting files) and for how many `days`.
async fn get_roots(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    blocking(move || {
        let (keep, days) = (encore_library::folders::trash_keep(&st.cfg), encore_library::folders::trash_days(&st.cfg));
        Ok(Json(Value::Array(roots_of(&st).iter().map(|r| {
            let mut v = r.json();
            if r.kind == roots::RootKind::Trash {
                v["keep"] = Value::Bool(keep);
                v["days"] = Value::from(days);
            }
            v
        }).collect())))
    }).await
}

/// `GET /api/files/list` query.
#[derive(Deserialize)]
struct ListQ {
    /// the root folder's key
    root: String,
    /// the folder under the root ("" = the root)
    #[serde(default)]
    path: String,
    /// name, size or mtime
    sort: Option<String>,
    /// asc or desc
    dir: Option<String>,
    /// where the page starts
    offset: Option<usize>,
    /// the page size
    limit: Option<usize>,
}

/// GET /api/files/list: a folder's entries, sorted and paged.
async fn list_dir(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<ListQ>) -> ApiResult<Json<Value>> {
    let query = ListQuery::parse(q.sort.as_deref(), q.dir.as_deref(), q.offset, q.limit)?;
    blocking(move || {
        let roots = roots_of(&st);
        let t = target(&roots, &q.root, &q.path)?;
        list::list(&st.db, &LibraryDirs::of(&st.cfg), &roots, &t, &query).map(Json)
    }).await
}

/// `GET /api/files/search` query.
#[derive(Deserialize)]
struct SearchQ {
    /// the root folder's key
    root: String,
    /// the folder searched from ("" = the whole root)
    #[serde(default)]
    path: String,
    /// the words a name must hold
    #[serde(default)]
    q: String,
    /// the most matches to give (default [`search::LIMIT`], at most [`search::MAX_LIMIT`])
    limit: Option<usize>,
}

/// GET /api/files/search: the files and folders under a folder whose names hold every word of `q`, at any
/// depth (encore_files::search). Trash is searched from its own list (`GET /api/files/trash`), not here.
async fn search_names(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<SearchQ>) -> ApiResult<Json<Value>> {
    blocking(move || {
        let roots = roots_of(&st);
        let t = target(&roots, &q.root, &q.path)?;
        if t.root.kind == roots::RootKind::Trash {
            return Err(err(400, "Search the trash from its list"));
        }
        search::search(&roots, &t, &q.q, q.limit).map(Json)
    }).await
}

/// `POST /api/files/mkdir` and `/rename` body.
#[derive(Deserialize)]
struct NameBody {
    /// the root folder's key
    root: String,
    /// the folder (mkdir) or the item (rename) under the root
    #[serde(default)]
    path: String,
    /// the new name
    name: String,
}

/// POST /api/files/mkdir: make a folder.
async fn mkdir(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<NameBody>) -> ApiResult<Json<Value>> {
    blocking(move || {
        let t = target(&roots_of(&st), &b.root, &b.path)?;
        ops::mkdir(&st, &Who::of(&u), &t, &b.name).map(Json)
    }).await
}

/// POST /api/files/rename: rename a file or folder (the library's rows follow).
async fn rename(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<NameBody>) -> ApiResult<Json<Value>> {
    blocking(move || {
        let t = target(&roots_of(&st), &b.root, &b.path)?;
        ops::rename(&st, &Who::of(&u), &t, &b.name).map(Json)
    }).await
}

/// `POST /api/files/move` body.
#[derive(Deserialize)]
struct MoveBody {
    /// the root the items are under
    root: String,
    /// the items' paths under it
    from: Vec<String>,
    /// the root to move them to
    to_root: String,
    /// the folder under it
    #[serde(default)]
    to_path: String,
}

/// POST /api/files/move: move items to another folder (the library's rows follow).
async fn move_items(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<MoveBody>) -> ApiResult<Json<Value>> {
    if b.from.is_empty() {
        return Err(err(400, "Nothing to move"));
    }
    blocking(move || {
        let roots = roots_of(&st);
        let to = target(&roots, &b.to_root, &b.to_path)?;
        let from = b.from.iter().map(|p| target(&roots, &b.root, p)).collect::<ApiResult<Vec<_>>>()?;
        ops::move_items(&st, &Who::of(&u), &from, &to).map(Json)
    }).await
}

/// `POST /api/files/trash` body.
#[derive(Deserialize)]
struct TrashBody {
    /// the root the items are under
    root: String,
    /// the items' paths under it
    paths: Vec<String>,
}

/// POST /api/files/trash: move items to the trash.
async fn to_trash(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<TrashBody>) -> ApiResult<Json<Value>> {
    if b.paths.is_empty() {
        return Err(err(400, "Nothing to move to trash"));
    }
    blocking(move || {
        let roots = roots_of(&st);
        let items = b.paths.iter().map(|p| target(&roots, &b.root, p)).collect::<ApiResult<Vec<_>>>()?;
        trash::trash(&st, &Who::of(&u), &items).map(Json)
    }).await
}

/// GET /api/files/trash: what the trash holds.
async fn trash_items(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    blocking(move || trash::list(&st).map(Json)).await
}

/// `POST /api/files/restore` body.
#[derive(Deserialize)]
struct RestoreBody {
    /// the trash items' ids
    items: Vec<String>,
}

/// POST /api/files/restore: put trash items back where they were.
async fn restore(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<RestoreBody>) -> ApiResult<Json<Value>> {
    blocking(move || trash::restore(&st, &Who::of(&u), &roots_of(&st), &b.items).map(Json)).await
}

/// DELETE /api/files/trash/{id}: delete one trash item for good.
async fn delete_forever(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if !trash::valid_id(&id) {
        return Err(err(404, "That isn't in the trash"));
    }
    blocking(move || trash::delete(&st, &Who::of(&u), &id).map(|_| StatusCode::NO_CONTENT)).await
}

/// POST /api/files/empty-trash: delete everything in the trash for good.
async fn empty_trash(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<StatusCode> {
    blocking(move || trash::empty(&st, &Who::of(&u)).map(|_| StatusCode::NO_CONTENT)).await
}

/// `GET /api/files/tags` query.
#[derive(Deserialize)]
struct FileQ {
    /// the root folder's key
    root: String,
    /// the file under it
    path: String,
}

/// GET /api/files/tags: a file's tags.
async fn get_tags(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<FileQ>) -> ApiResult<Json<Value>> {
    blocking(move || tags::get(&target(&roots_of(&st), &q.root, &q.path)?).map(Json)).await
}

/// `PUT /api/files/tags` body.
#[derive(Deserialize)]
struct TagsBody {
    /// the root folder's key
    root: String,
    /// the file under it
    path: String,
    /// the tags to write, by name
    tags: Map<String, Value>,
}

/// PUT /api/files/tags: write a file's tags (its library row follows).
async fn put_tags(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<TagsBody>) -> ApiResult<Json<Value>> {
    blocking(move || {
        let t = target(&roots_of(&st), &b.root, &b.path)?;
        tags::put(&st, &Who::of(&u), &t, &b.tags).map(Json)
    }).await
}

/// `POST /api/files/upload` query.
#[derive(Deserialize)]
struct UploadQ {
    /// the root folder's key
    root: String,
    /// the folder under it
    #[serde(default)]
    path: String,
    /// rename (the default), replace or skip a file that is there
    conflict: Option<String>,
}

/// The library's rows of an upload's finished files, written once when the upload ends. When the request is
/// dropped part way (the client went away), whoever holds it last – the request or a finishing file's
/// blocking step – writes what is left, so a file that landed is never left without its row.
struct PendingRows {
    /// the server
    st: AppState,
    /// the rows still to write
    change: std::sync::Mutex<Change>,
}

impl PendingRows {
    /// The rows still to write, taking them.
    fn take(&self) -> Change {
        std::mem::take(&mut *self.change.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for PendingRows {
    fn drop(&mut self) {
        let change = self.take();
        if change.is_empty() {
            return;
        }
        let st = self.st.clone();
        let apply = move || {
            if let Err(e) = change.apply(&st) {
                tracing::warn!("files: reading in uploaded songs: {e:#}");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => drop(h.spawn_blocking(apply)),
            Err(_) => apply(),
        }
    }
}

/// `POST /api/files/upload?root=&path=&conflict=`: multipart `file` parts, each streamed to a part file of its
/// own and renamed when complete (see encore_files::upload).
async fn upload_files(State(st): State<AppState>, Admin(u): Admin, Query(q): Query<UploadQ>, mut form: Multipart) -> ApiResult<Json<Value>> {
    use tokio::io::AsyncWriteExt;
    let who = Who::of(&u);
    let conflict = Conflict::parse(q.conflict.as_deref())?;
    let base = {
        let st = st.clone();
        blocking(move || {
            let t = target(&roots_of(&st), &q.root, &q.path)?;
            t.open_dir()?;
            Ok(t)
        }).await?
    };
    let mut outcomes: Vec<Outcome> = vec![];
    let pending = std::sync::Arc::new(PendingRows { st: st.clone(), change: std::sync::Mutex::new(Change::default()) });
    let stopped = loop {
        let mut field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break None,
            Err(e) => break Some(err(400, format!("The upload stopped: {e}"))),
        };
        // a part without a filename is a form field, not a file
        let Some(sent) = field.file_name().map(String::from) else { continue };
        let begun = {
            let (b, s) = (base.clone(), sent.clone());
            blocking(move || upload::begin(&b, &s, conflict)).await
        };
        let inc = match begun {
            Ok(Begin::Write(inc)) => *inc,
            // its bytes are passed over by the next look for a part
            Ok(Begin::Skip(sent, reason)) => {
                outcomes.push(Outcome::Skipped { sent, reason });
                continue;
            }
            // this file can't go there (a folder that can't be made, say): the others still can
            Err(e) => {
                outcomes.push(Outcome::Skipped { sent, reason: e.detail });
                continue;
            }
        };
        let file = match inc.file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                let _ = tokio::task::spawn_blocking(move || upload::abort(inc)).await;
                break Some(err(500, format!("Couldn't write {sent}: {e}")));
            }
        };
        let mut out = tokio::fs::File::from_std(file);
        let mut size: u64 = 0;
        // Err(true): the client went away (the whole upload stops); Err(false): this file couldn't be written
        let written: Result<(), (bool, String)> = async {
            while let Some(chunk) = field.chunk().await.map_err(|e| (true, format!("The upload stopped: {e}")))? {
                size += chunk.len() as u64;
                out.write_all(&chunk).await.map_err(|e| (false, format!("Couldn't write {sent}: {e}")))?;
            }
            out.flush().await.map_err(|e| (false, format!("Couldn't write {sent}: {e}")))?;
            out.sync_all().await.map_err(|e| (false, format!("Couldn't write {sent}: {e}")))?;
            Ok(())
        }.await;
        drop(out);
        match written {
            Ok(()) => {
                let (rows, who2) = (pending.clone(), who.clone());
                let o = blocking(move || {
                    let mut ch = rows.change.lock().unwrap_or_else(|e| e.into_inner());
                    Ok(upload::finish(&rows.st, &who2, inc, size, conflict, &mut ch))
                }).await?;
                outcomes.push(o);
            }
            Err((gone, why)) => {
                let _ = tokio::task::spawn_blocking(move || upload::abort(inc)).await;
                if gone {
                    break Some(err(400, why));
                }
                outcomes.push(Outcome::Skipped { sent, reason: why });
            }
        }
    };
    // what arrived in the library is read in, also when the upload stopped part way
    let (st2, change) = (st.clone(), pending.take());
    let _ = blocking(move || {
        if let Err(e) = change.apply(&st2) {
            tracing::warn!("files: reading in uploaded songs: {e:#}");
        }
        Ok(())
    }).await;
    match stopped {
        Some(e) if outcomes.is_empty() => Err(e),
        _ => Ok(Json(upload::answer(&outcomes))),
    }
}

/// `POST /api/files/import/preview` body.
#[derive(Deserialize)]
struct PreviewBody {
    /// the root folder's key
    root: String,
    /// the folder to import
    #[serde(default)]
    path: String,
    /// "move" (the default) or "in_place"
    #[serde(default)]
    mode: String,
    /// a release to match the folder's album to
    release_id: Option<String>,
}

/// POST /api/files/import/preview: how a folder would be imported: its albums, their matches and the new paths.
async fn import_preview(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<PreviewBody>) -> ApiResult<Json<Value>> {
    let mode = Mode::parse(&b.mode)?;
    let t = target(&roots_of(&st), &b.root, &b.path)?;
    import::preview(&st, &t, mode, b.release_id.as_deref().filter(|r| !r.is_empty())).await.map(Json)
}

/// `POST /api/files/import/match` body.
#[derive(Deserialize)]
struct MatchBody {
    /// the root folder's key
    root: String,
    /// the album's folder under it
    path: String,
    /// the artist to search for (default: the files' tags)
    #[serde(default)]
    artist: String,
    /// the album to search for (default: the files' tags)
    #[serde(default)]
    album: String,
    /// the metadata source (default MusicBrainz)
    provider: Option<String>,
}

/// POST /api/files/import/match: releases that may be a folder's album.
async fn import_match(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<MatchBody>) -> ApiResult<Json<Value>> {
    let t = target(&roots_of(&st), &b.root, &b.path)?;
    let f = {
        let t2 = t.clone();
        blocking(move || import::folder(&t2, &t2.rel())).await?
    };
    let provider = b.provider.unwrap_or_else(|| "musicbrainz".into());
    import::candidates(&st, &f, &b.artist, &b.album, &provider).await.map(Json)
}

/// One album of an import: its folder and the release to tag it with.
#[derive(Deserialize)]
struct ApplyAlbum {
    /// the album's folder
    folder: String,
    /// the release chosen (none: tag as it is)
    release_id: Option<String>,
}

/// `POST /api/files/import/apply` body.
#[derive(Deserialize)]
struct ApplyBody {
    /// the root folder's key
    root: String,
    /// the folder to import
    #[serde(default)]
    path: String,
    /// "move" (the default) or "in_place"
    #[serde(default)]
    mode: String,
    /// the albums and their releases
    albums: Vec<ApplyAlbum>,
}

/// POST /api/files/import/apply: import a folder's albums into the library.
async fn import_apply(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<ApplyBody>) -> ApiResult<Json<Value>> {
    let mode = Mode::parse(&b.mode)?;
    let t = target(&roots_of(&st), &b.root, &b.path)?;
    let albums: Vec<(String, Option<String>)> = b.albums.into_iter().map(|a| (a.folder, a.release_id)).collect();
    import::apply(&st, &Who::of(&u), &t, mode, &albums).await.map(Json)
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
