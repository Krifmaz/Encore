//! Bring your own music (the routes): replace one song's file with your own (⋯ → Replace with my own file),
//! or upload a batch of files that are matched to the songs they are by their tags and replace them once
//! you confirm (Library → Upload your own files). Admins, and accounts with the `own_files` permission:
//! a replacement changes the song for everyone. The work is encore_library::own_file's.
//!
//! - `POST /api/library/tracks/{id}/own-file` (multipart, one `file` part): replace that song's file now.
//! - `POST /api/library/own-files` (multipart, any number of `file` parts): stage them, each with the song it
//!   matches → `{files: [{token, name, size, format, tags, match, candidates}], failed: [{name, reason}]}`.
//! - `POST /api/library/own-files/apply` `{items: [{token, track_id}]}`: replace those songs' files with the
//!   staged uploads → `{replaced: [{token, track_id, title, artist, warnings}], failed: [{token, reason}]}`.
//! - `DELETE /api/library/own-files/{token}`: drop a staged upload that won't be used.
use super::files::UPLOAD_LIMIT;
use crate::auth::User;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::routing::{delete, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiError, ApiResult};
use encore_db::{Row, RowExt};
use encore_library::own_file::{self, Refused};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The permission that lets an account replace songs' files (admins always may).
pub const PERM: &str = "own_files";

/// The routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/library/tracks/{id}/own-file", post(replace_one).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/library/own-files", post(stage_many).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/library/own-files/apply", post(apply))
        .route("/api/library/own-files/{token}", delete(drop_staged))
}

/// A refusal of encore_library::own_file as the API's error.
fn api(r: Refused) -> ApiError {
    err(r.status, r.detail)
}

/// May `u` replace songs' files? Admins, and accounts given the permission.
fn allowed(st: &AppState, u: &Row) -> ApiResult<()> {
    if encore_config::perms::can(&st.cfg, u, PERM) {
        Ok(())
    } else {
        Err(err(403, "Replacing songs with your own files needs the “Replace songs with their own files” permission – ask an admin"))
    }
}

/// Streams the next `file` part of the form into the staging folder. → `Some((token, name, path))` for a file,
/// `Ok(None)` when the form has no more parts; `Err` when the part can't be taken: `(stop, reason, name)`,
/// where `stop` says the upload itself broke off (the rest can't be read).
async fn next_file(st: &AppState, form: &mut Multipart) -> Result<Option<(String, String, PathBuf)>, (bool, String, String)> {
    use tokio::io::AsyncWriteExt;
    loop {
        let mut field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => return Ok(None),
            Err(e) => return Err((true, format!("The upload stopped: {e}"), String::new())),
        };
        // a part without a filename is a form field, not a file
        let Some(sent) = field.file_name().map(String::from) else { continue };
        // a folder upload's "Album/01 Song.flac": the name is the last part
        let name = sent.rsplit(['/', '\\']).next().unwrap_or(&sent).to_string();
        let (token, path) = match own_file::stage_new(st, &name) {
            Ok(x) => x,
            // its bytes are passed over by the next look for a part
            Err(r) => return Err((false, r.detail, name)),
        };
        let res: Result<(), (bool, String)> = async {
            let mut out = tokio::fs::File::create(&path).await.map_err(|e| (false, format!("Couldn't keep “{name}”: {e}")))?;
            while let Some(chunk) = field.chunk().await.map_err(|e| (true, format!("The upload stopped: {e}")))? {
                out.write_all(&chunk).await.map_err(|e| (false, format!("Couldn't keep “{name}”: {e}")))?;
            }
            out.flush().await.map_err(|e| (false, format!("Couldn't keep “{name}”: {e}")))?;
            Ok(())
        }.await;
        if let Err((stop, why)) = res {
            let _ = tokio::fs::remove_file(&path).await;
            return Err((stop, why, name));
        }
        return Ok(Some((token, name, path)));
    }
}

/// `POST /api/library/tracks/{id}/own-file`: the first `file` part replaces the song's file → see
/// encore_library::own_file::replace.
async fn replace_one(State(st): State<AppState>, User(u): User, Path(id): Path<i64>, mut form: Multipart) -> ApiResult<Json<Value>> {
    allowed(&st, &u)?;
    if st.db.one("SELECT 1 FROM tracks WHERE id=?", encore_db::p![id])?.is_none() {
        return Err(err(404, "Song not found"));
    }
    let (token, name, path) = match next_file(&st, &mut form).await {
        Ok(Some(f)) => f,
        Ok(None) => return Err(err(400, "Choose a file to upload")),
        Err((_, why, _)) => return Err(err(400, why)),
    };
    let s2 = st.clone();
    let res = tokio::task::spawn_blocking(move || {
        // checked to be audio first (a file that isn't is removed and refused)
        own_file::stage_done(&s2, u.i("id"), &token, &name, &path)?;
        let out = own_file::replace(&s2, &u, id, &path, &name);
        if out.is_err() {
            own_file::unstage(&s2, &token);
        }
        out
    }).await.map_err(|e| err(500, format!("The replacement stopped: {e}")))?;
    let mut v = res.map_err(api)?;
    quality_check(&st, id, &mut v).await;
    Ok(Json(v))
}

/// Runs the quality checks (fake lossless, upsampled, very quiet …) on a replaced song when they run on
/// imports (Settings), and puts what they found into the answer as `quality`. They only flag: the file is
/// the person's own choice, so nothing is turned away.
async fn quality_check(st: &AppState, id: i64, v: &mut Value) {
    if !encore_media::quality::on_import(st) {
        return;
    }
    let Some(t) = st.db.one("SELECT path FROM tracks WHERE id=?", encore_db::p![id]).ok().flatten() else { return };
    let path = PathBuf::from(t.s("path"));
    let rep = encore_media::quality::analyze(&st.media, &path, None, encore_media::quality::min_lufs(st), false).await;
    encore_media::quality::store(st, &path, &rep);
    v["quality"] = encore_media::quality::for_track(st, id);
}

/// `POST /api/library/own-files`: every `file` part is staged and matched (see the module's notes).
async fn stage_many(State(st): State<AppState>, User(u): User, mut form: Multipart) -> ApiResult<Json<Value>> {
    allowed(&st, &u)?;
    let (mut files, mut failed) = (vec![], vec![]);
    loop {
        match next_file(&st, &mut form).await {
            Ok(Some((token, name, path))) => {
                let (s2, uid) = (st.clone(), u.i("id"));
                let done = tokio::task::spawn_blocking(move || {
                    let staged = own_file::stage_done(&s2, uid, &token, &name, &path).map_err(|r| (name.clone(), r.detail))?;
                    Ok::<_, (String, String)>(own_file::staged_with_match(&s2, staged, &path))
                }).await.map_err(|e| err(500, format!("The upload stopped: {e}")))?;
                match done {
                    Ok(v) => files.push(v),
                    Err((name, reason)) => failed.push(json!({ "name": name, "reason": reason })),
                }
            }
            Ok(None) => break,
            Err((stop, reason, name)) => {
                failed.push(json!({ "name": name, "reason": reason }));
                if stop {
                    break;
                }
            }
        }
    }
    if files.is_empty() && failed.is_empty() {
        return Err(err(400, "Choose the files to upload"));
    }
    Ok(Json(json!({ "files": files, "failed": failed })))
}

/// `POST /api/library/own-files/apply` body.
#[derive(Deserialize)]
struct ApplyBody {
    /// the staged uploads and the song each replaces
    items: Vec<ApplyItem>,
}

/// One staged upload and the song it replaces.
#[derive(Deserialize)]
struct ApplyItem {
    /// the staged upload's token
    token: String,
    /// the song whose file it replaces
    track_id: i64,
}

/// `POST /api/library/own-files/apply`: each staged upload replaces its song's file, one after the other.
async fn apply(State(st): State<AppState>, User(u): User, Json(b): Json<ApplyBody>) -> ApiResult<Json<Value>> {
    allowed(&st, &u)?;
    // one file for one song: the same song twice would trash the first upload at once
    let mut seen = std::collections::HashSet::new();
    if b.items.iter().any(|i| !seen.insert(i.track_id)) {
        return Err(err(400, "Each song can take one file"));
    }
    let (mut replaced, mut failed) = (vec![], vec![]);
    for it in b.items {
        let (s2, u2, token) = (st.clone(), u.clone(), it.token.clone());
        let res = tokio::task::spawn_blocking(move || {
            let (path, name) = own_file::staged(&s2, u2.i("id"), &token)?;
            own_file::replace(&s2, &u2, it.track_id, &path, &name)
        }).await.map_err(|e| err(500, format!("The replacement stopped: {e}")))?;
        match res {
            Ok(mut v) => {
                quality_check(&st, it.track_id, &mut v).await;
                replaced.push(json!({ "token": it.token, "track_id": it.track_id, "title": v["track"]["title"], "artist": v["track"]["artist"],
                                      "format": v["after"]["format"], "warnings": v["warnings"], "quality": v.get("quality").cloned().unwrap_or(Value::Null) }));
            }
            Err(r) => failed.push(json!({ "token": it.token, "track_id": it.track_id, "reason": r.detail })),
        }
    }
    Ok(Json(json!({ "replaced": replaced, "failed": failed })))
}

/// `DELETE /api/library/own-files/{token}`: a staged upload of yours is dropped.
async fn drop_staged(State(st): State<AppState>, User(u): User, Path(token): Path<String>) -> ApiResult<Json<Value>> {
    allowed(&st, &u)?;
    match own_file::staged(&st, u.i("id"), &token) {
        Ok(_) => own_file::unstage(&st, &token),
        // already gone is fine; someone else's is not
        Err(r) if r.status == 403 => return Err(api(r)),
        Err(_) => {}
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
#[path = "own_file_tests.rs"]
mod tests;
