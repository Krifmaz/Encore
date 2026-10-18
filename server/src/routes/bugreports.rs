//! Bug reports (side bar → "Bug reports"). A user writes what went wrong, attaches screenshots or a log, and
//! the admin answers in a thread; the admin can export the lot as a zip to hand to the developer.
//!
//! POST   /api/bug-reports                      multipart kind, title, body, page, app_version, device, file… → {ok, id}
//! GET    /api/bug-reports?status=&kind=&q=&mine=1 → {reports: [{id, kind, title, status, username, user_id, created_at,
//!                                              updated_at, message_count, file_count, unread}]}
//! GET    /api/bug-reports/unread               → {count}
//! GET    /api/bug-reports/export?status=&kind=&ids=  (admin) → application/zip (reports.md, reports.json, files/…)
//! GET    /api/bug-reports/{id}                 → {report: {…, files: […]}, messages: [{…, files: […]}]}
//! POST   /api/bug-reports/{id}/messages        multipart body, file… → {ok, id}
//! POST   /api/bug-reports/{id}/status          {status} (admin) → {ok}
//! DELETE /api/bug-reports/{id}                 (admin) → {ok}
//! GET    /api/bug-reports/{id}/files/{fid}     → the file
//!
//! A non-admin only sees their own reports; someone else's answers 404, not 403. The files live under
//! <data_dir>/reports/<report_id>/<file_id>.<ext>; a path on disk is only ever built from a database row.
use crate::auth::{self, Admin, User};
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiError, ApiResult};
use crate::state::AppState;
use crate::{email, notify, p};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

/// Bytes per attached file.
pub const MAX_FILE: usize = 8 * 1024 * 1024;
/// Files per report or per message.
pub const MAX_FILES: usize = 6;
/// The whole upload: the files plus the text.
const UPLOAD_LIMIT: usize = MAX_FILES * MAX_FILE + 2 * 1024 * 1024;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 20_000;
const MAX_NAME: usize = 120;
/// What an image may decode to: 128 MiB holds a 5K screenshot (5120×2880 RGBA8 is 56 MB, RGBA16 twice that), and a
/// 400 KB PNG claiming 11000×11000 is refused instead of taking half a gigabyte.
const MAX_PIXEL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SIDE: u32 = 8192;
/// Reports a user may file, and replies they may send, in an hour.
const REPORTS_PER_HOUR: usize = 20;
const REPLIES_PER_HOUR: usize = 60;
/// Bytes of files one user's reports may hold in all (admins are not held back).
const MAX_USER_BYTES: i64 = 500 * 1024 * 1024;
/// Files an export may hold: the zip is built in memory.
const MAX_EXPORT_BYTES: i64 = 512 * 1024 * 1024;

pub const KINDS: [&str; 3] = ["bug", "not_bug", "howto"];
pub const STATUSES: [&str; 5] = ["new", "looking", "fixed", "wontfix", "answered"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/bug-reports", get(list).post(create).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/bug-reports/unread", get(unread))
        .route("/api/bug-reports/export", get(export))
        .route("/api/bug-reports/{id}", get(detail).delete(remove))
        .route("/api/bug-reports/{id}/messages", post(reply).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/bug-reports/{id}/status", post(set_status))
        .route("/api/bug-reports/{id}/files/{fid}", get(file))
}

fn ensure_table(st: &AppState) -> ApiResult<()> {
    // created on first use: an older server simply never reads these
    st.db.execute_batch(
        "CREATE TABLE IF NOT EXISTS bug_reports (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER,
            username TEXT,
            kind TEXT,
            title TEXT,
            body TEXT,
            page TEXT,
            app_version TEXT,
            device TEXT,
            status TEXT DEFAULT 'new',
            created_at REAL,
            updated_at REAL,
            last_reply_by TEXT DEFAULT '',
            user_seen_at REAL DEFAULT 0,
            admin_seen_at REAL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS bug_report_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            report_id INTEGER,
            user_id INTEGER,
            username TEXT,
            is_admin INTEGER,
            body TEXT,
            created_at REAL
        );
        CREATE TABLE IF NOT EXISTS bug_report_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            report_id INTEGER,
            message_id INTEGER,
            name TEXT,
            mime TEXT,
            size INTEGER,
            path TEXT,
            created_at REAL
        );
        CREATE INDEX IF NOT EXISTS idx_bug_report_messages_report ON bug_report_messages(report_id);
        CREATE INDEX IF NOT EXISTS idx_bug_report_files_report ON bug_report_files(report_id);",
    )?;
    Ok(())
}

fn reports_dir(st: &AppState) -> PathBuf {
    st.data_dir().join("reports")
}

/// What the side bar and the export call a kind.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "not_bug" => "Not a bug",
        "howto" => "How do I…",
        _ => "Bug",
    }
}

/// What the list and the export call a status.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "looking" => "Looking into it",
        "fixed" => "Fixed",
        "wontfix" => "Won't fix",
        "answered" => "Answered",
        _ => "New",
    }
}

/// `YYYY-MM-DD` of a unix time (UTC), without a date crate (Howard Hinnant's civil_from_days).
pub fn ymd(secs: f64) -> String {
    let z = (secs.max(0.0) as i64).div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM UTC` of a unix time.
fn when(secs: f64) -> String {
    let t = (secs.max(0.0) as i64).rem_euclid(86400);
    format!("{} {:02}:{:02} UTC", ymd(secs), t / 3600, (t % 3600) / 60)
}

/// The first `max` characters of `s`, trimmed.
fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// `s` on one line: control characters (newlines among them) become spaces and runs of white space one space, so a
/// title can't add lines to the export's Markdown, the log or a notification.
fn line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- uploads -----------------------------------------------------------------------------------

/// A file taken from the form, checked and ready to be written.
struct Pending {
    name: String,
    mime: &'static str,
    ext: &'static str,
    data: Vec<u8>,
}

struct Form {
    fields: HashMap<String, String>,
    files: Vec<Pending>,
}

impl Form {
    fn text(&self, key: &str) -> &str {
        self.fields.get(key).map(|s| s.as_str()).unwrap_or("")
    }
}

/// The file's own name without any folder parts, with the characters a file system refuses replaced and the
/// extension the bytes were sniffed as (never the one the browser sent: `readme.html` holding a PNG is stored as
/// `readme.png`), cut to 120 characters; `file.<ext>` when nothing is left.
pub fn safe_name(name: &str, ext: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !c.is_control()).map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.');
    let stem = clean.rsplit_once('.').map(|(s, _)| s).unwrap_or(clean).trim().trim_start_matches('.');
    let stem: String = stem.chars().take(MAX_NAME - ext.len() - 1).collect();
    if stem.trim().is_empty() { format!("file.{ext}") } else { format!("{}.{ext}", stem.trim()) }
}

/// What the bytes are – never what the browser said. Images are sniffed and decoded with the `image` crate, so a
/// broken file is refused, and so is one that would decode to more than MAX_PIXEL_BYTES or MAX_SIDE a side (the
/// check happens on the header, before anything that big is allocated); text files are taken by their name (.txt,
/// .log) when they are UTF-8.
pub fn check_file(name: &str, data: &[u8]) -> ApiResult<(&'static str, &'static str)> {
    use image::ImageFormat as F;
    if let Ok(format) = image::guess_format(data) {
        let (mime, ext) = match format {
            F::Png => ("image/png", "png"),
            F::Jpeg => ("image/jpeg", "jpg"),
            F::WebP => ("image/webp", "webp"),
            F::Gif => ("image/gif", "gif"),
            _ => return Err(err(400, "Only PNG, JPEG, WebP or GIF images and .txt or .log text files can be attached")),
        };
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_PIXEL_BYTES);
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        reader.limits(limits);
        reader.decode().map_err(|_| err(400, "That image can't be read"))?;
        return Ok((mime, ext));
    }
    let lower = name.to_ascii_lowercase();
    let ext = if lower.ends_with(".log") { "log" } else if lower.ends_with(".txt") { "txt" } else {
        return Err(err(400, "Only PNG, JPEG, WebP or GIF images and .txt or .log text files can be attached"));
    };
    if std::str::from_utf8(data).is_err() {
        return Err(err(400, "A text file has to be UTF-8"));
    }
    Ok(("text/plain", ext))
}

fn form_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        err(400, format!("That upload is too big: at most {MAX_FILES} files of {} MB each", MAX_FILE >> 20))
    } else {
        err(400, "Could not read the upload")
    }
}

/// Uploads being decoded at once: each may hold MAX_PIXEL_BYTES, so two at a time is what a small server can take.
static DECODES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Reads the whole form: text fields into `fields`, every `file` part (or any part with a file name) checked
/// and sized. Too many or too big files, and files of a kind we don't take, are refused with a sentence.
async fn read_form(mut form: Multipart) -> ApiResult<Form> {
    let mut fields = HashMap::new();
    let mut raw: Vec<(String, Vec<u8>)> = vec![];
    while let Some(mut f) = form.next_field().await.map_err(form_error)? {
        let field = f.name().unwrap_or("").to_string();
        let file_name = f.file_name().map(|s| s.to_string());
        if field == "file" || file_name.is_some() {
            if raw.len() >= MAX_FILES {
                return Err(err(400, format!("At most {MAX_FILES} files per report")));
            }
            let mut data = Vec::new();
            while let Some(chunk) = f.chunk().await.map_err(form_error)? {
                if data.len() + chunk.len() > MAX_FILE {
                    return Err(err(400, format!("Each file can be at most {} MB", MAX_FILE >> 20)));
                }
                data.extend_from_slice(&chunk);
            }
            if data.is_empty() {
                continue; // a file input left empty
            }
            raw.push((file_name.unwrap_or_default(), data));
        } else {
            let mut data = Vec::new();
            while let Some(chunk) = f.chunk().await.map_err(form_error)? {
                if data.len() + chunk.len() > 4 * MAX_BODY {
                    return Err(err(400, "A text field is too long"));
                }
                data.extend_from_slice(&chunk);
            }
            fields.insert(field, String::from_utf8_lossy(&data).into_owned());
        }
    }
    if raw.is_empty() {
        return Ok(Form { fields, files: vec![] });
    }
    // decoding a few 8 MB images is not for the request thread, and not for many threads at once either
    let _permit = DECODES.acquire().await.map_err(|_| err(500, "Uploads are not being checked right now"))?;
    let files = tokio::task::spawn_blocking(move || -> ApiResult<Vec<Pending>> {
        raw.into_iter().map(|(name, data)| {
            let (mime, ext) = check_file(&name, &data)?;
            Ok(Pending { name: safe_name(&name, ext), mime, ext, data })
        }).collect()
    })
    .await
    .map_err(|e| err(500, e.to_string()))??;
    Ok(Form { fields, files })
}

/// The files a user's reports may hold in all: refuses `files` when they would take the user over MAX_USER_BYTES.
/// Admins are not held back.
fn check_quota(st: &AppState, u: &Row, files: &[Pending]) -> ApiResult<()> {
    if files.is_empty() || auth::is_admin(u) {
        return Ok(());
    }
    let held = st.db.one("SELECT COALESCE(SUM(f.size),0) n FROM bug_report_files f JOIN bug_reports r ON r.id=f.report_id WHERE r.user_id=?", p![u.i("id")])?
        .map(|r| r.i("n")).unwrap_or(0);
    let adding: i64 = files.iter().map(|f| f.data.len() as i64).sum();
    if held + adding > MAX_USER_BYTES {
        return Err(err(400, format!("Your reports already hold {} MB of files (the most is {} MB); the admin can delete old ones", held >> 20, MAX_USER_BYTES >> 20)));
    }
    Ok(())
}

/// Writes the files under <data_dir>/reports/<report_id>/ and records them.
async fn store_files(st: &AppState, report_id: i64, message_id: Option<i64>, files: Vec<Pending>) -> ApiResult<()> {
    if files.is_empty() {
        return Ok(());
    }
    let dir = reports_dir(st).join(report_id.to_string());
    tokio::fs::create_dir_all(&dir).await.map_err(|e| err(500, format!("Could not save the file: {e}")))?;
    for f in files {
        let fid = st.db.execute(
            "INSERT INTO bug_report_files (report_id, message_id, name, mime, size, path, created_at) VALUES (?,?,?,?,?,?,?)",
            p![report_id, message_id, f.name, f.mime, f.data.len(), "", now()],
        )?;
        let file_name = format!("{fid}.{}", f.ext);
        tokio::fs::write(dir.join(&file_name), &f.data).await.map_err(|e| err(500, format!("Could not save the file: {e}")))?;
        st.db.execute("UPDATE bug_report_files SET path=? WHERE id=?", p![format!("{report_id}/{file_name}"), fid])?;
    }
    Ok(())
}

// ---- reading -----------------------------------------------------------------------------------

/// The report, or 404 – also when it is someone else's and the viewer is no admin.
fn load(st: &AppState, id: i64, u: &Row) -> ApiResult<Row> {
    let r = st.db.one("SELECT * FROM bug_reports WHERE id=?", p![id])?.ok_or_else(|| err(404, "Report not found"))?;
    if !auth::is_admin(u) && r.i("user_id") != u.i("id") {
        return Err(err(404, "Report not found"));
    }
    Ok(r)
}

/// A report with its files and its thread.
struct Full {
    report: Row,
    files: Vec<Row>,
    messages: Vec<(Row, Vec<Row>)>,
}

fn load_full(st: &AppState, report: Row) -> ApiResult<Full> {
    let id = report.i("id");
    let all = st.db.query("SELECT id, message_id, name, mime, size, path FROM bug_report_files WHERE report_id=? ORDER BY id", p![id])?;
    let files = all.iter().filter(|f| f.get("message_id").is_none_or(Value::is_null)).cloned().collect();
    let messages = st.db.query("SELECT id, user_id, username, is_admin, body, created_at FROM bug_report_messages WHERE report_id=? ORDER BY id", p![id])?
        .into_iter()
        .map(|m| {
            let mine = all.iter().filter(|f| f.i("message_id") == m.i("id")).cloned().collect();
            (m, mine)
        })
        .collect();
    Ok(Full { report, files, messages })
}

fn file_json(report_id: i64, f: &Row) -> Value {
    json!({ "id": f.i("id"), "name": f.s("name"), "mime": f.s("mime"), "size": f.i("size"), "url": format!("/api/bug-reports/{report_id}/files/{}", f.i("id")) })
}

fn message_json(report_id: i64, m: &Row, files: &[Row]) -> Value {
    json!({ "id": m.i("id"), "username": m.s("username"), "is_admin": m.b("is_admin"), "body": m.s("body"), "created_at": m.f("created_at"),
            "files": files.iter().map(|f| file_json(report_id, f)).collect::<Vec<_>>() })
}

fn report_json(full: &Full) -> Value {
    let r = &full.report;
    let id = r.i("id");
    json!({
        "report": {
            "id": id, "kind": r.s("kind"), "title": r.s("title"), "body": r.s("body"), "page": r.s("page"), "app_version": r.s("app_version"),
            "device": r.s("device"), "status": r.s("status"), "username": r.s("username"), "user_id": r.i("user_id"),
            "created_at": r.f("created_at"), "updated_at": r.f("updated_at"),
            "files": full.files.iter().map(|f| file_json(id, f)).collect::<Vec<_>>(),
        },
        "messages": full.messages.iter().map(|(m, files)| message_json(id, m, files)).collect::<Vec<_>>(),
    })
}

const LAST_MESSAGE: &str = "COALESCE((SELECT MAX(m.created_at) FROM bug_report_messages m WHERE m.report_id=r.id), 0)";

/// The SQL of "this report has something the viewer hasn't seen": for an admin a report nobody looked at
/// yet, or a user's reply after the last look; for the reporter an admin's reply after the last look.
fn unread_sql(admin: bool) -> String {
    if admin {
        format!("(r.admin_seen_at < r.created_at OR (r.last_reply_by='user' AND {LAST_MESSAGE} > r.admin_seen_at))")
    } else {
        format!("(r.last_reply_by='admin' AND {LAST_MESSAGE} > r.user_seen_at)")
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ListQ {
    status: String,
    kind: String,
    q: String,
    mine: String,
    /// export only: `1,2,3`
    ids: String,
}

/// WHERE … for a list: the filters that are set, and the viewer's own reports unless an admin asks for all.
fn filter_sql(q: &ListQ, u: &Row, admin: bool) -> (String, Vec<Value>) {
    let mut where_ = vec!["1=1".to_string()];
    let mut params = p![];
    if STATUSES.contains(&q.status.as_str()) {
        where_.push("r.status=?".into());
        params.push(json!(q.status));
    }
    if KINDS.contains(&q.kind.as_str()) {
        where_.push("r.kind=?".into());
        params.push(json!(q.kind));
    }
    let search = q.q.trim();
    if !search.is_empty() {
        where_.push("(r.title LIKE ? ESCAPE '\\' OR r.body LIKE ? ESCAPE '\\' OR r.username LIKE ? ESCAPE '\\')".into());
        let like = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        params.extend([json!(like), json!(like), json!(like)]);
    }
    if !admin || matches!(q.mine.as_str(), "1" | "true") {
        where_.push("r.user_id=?".into());
        params.push(json!(u.i("id")));
    }
    let ids: Vec<i64> = q.ids.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if !ids.is_empty() {
        where_.push(format!("r.id IN ({})", vec!["?"; ids.len()].join(",")));
        params.extend(ids.into_iter().map(Value::from));
    }
    (where_.join(" AND "), params)
}

// ---- handlers ----------------------------------------------------------------------------------

pub async fn create(State(st): State<AppState>, User(u): User, form: Multipart) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let admin = auth::is_admin(&u);
    if !admin && st.limits.limited(&format!("bugreport:{}", u.i("id")), REPORTS_PER_HOUR, 3600.0, true) {
        return Err(err(429, "Too many reports in a short time – try again later"));
    }
    let form = read_form(form).await?;
    let title = line(form.text("title"));
    let body = form.text("body").trim().to_string();
    if title.is_empty() {
        return Err(err(400, "Give the report a title"));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(err(400, format!("The title can have at most {MAX_TITLE} characters")));
    }
    if body.is_empty() {
        return Err(err(400, "Describe what happened"));
    }
    if body.chars().count() > MAX_BODY {
        return Err(err(400, format!("The description can have at most {MAX_BODY} characters")));
    }
    let kind = form.text("kind").trim();
    let kind = if kind.is_empty() { "bug" } else if KINDS.contains(&kind) { kind } else {
        return Err(err(400, "Pick a kind: Bug, Not a bug or How do I…"));
    };
    check_quota(&st, &u, &form.files)?;
    let t = now();
    // an admin's own report is not news to them
    let admin_seen = if admin { t } else { 0.0 };
    let id = st.db.execute(
        "INSERT INTO bug_reports (user_id, username, kind, title, body, page, app_version, device, status, created_at, updated_at, last_reply_by, user_seen_at, admin_seen_at)
         VALUES (?,?,?,?,?,?,?,?,'new',?,?,'',?,?)",
        p![u.i("id"), u.s("username"), kind, title, body, cut(&line(form.text("page")), 300), cut(&line(form.text("app_version")), 60), cut(&line(form.text("device")), 300), t, t, t, admin_seen],
    )?;
    store_files(&st, id, None, form.files).await?;
    st.db.log("info", &format!("{} filed bug report #{id}: {title}", u.s("username")));
    let (st2, who) = (st.clone(), u.s("username"));
    tokio::spawn(async move { notify::send(&st2, "New bug report", &format!("{who}: {title}"), None).await });
    Ok(Json(json!({ "ok": true, "id": id })))
}

pub async fn list(State(st): State<AppState>, User(u): User, Query(q): Query<ListQ>) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let admin = auth::is_admin(&u);
    let (where_, params) = filter_sql(&q, &u, admin);
    let rows = st.db.query(
        &format!(
            "SELECT r.id, r.kind, r.title, r.status, r.username, r.user_id, r.created_at, r.updated_at,
                    (SELECT COUNT(*) FROM bug_report_messages m WHERE m.report_id=r.id) message_count,
                    (SELECT COUNT(*) FROM bug_report_files f WHERE f.report_id=r.id) file_count,
                    {} unread
             FROM bug_reports r WHERE {where_} ORDER BY r.updated_at DESC, r.id DESC",
            unread_sql(admin)
        ),
        params,
    )?;
    let reports: Vec<Value> = rows.iter().map(|r| json!({
        "id": r.i("id"), "kind": r.s("kind"), "title": r.s("title"), "status": r.s("status"), "username": r.s("username"), "user_id": r.i("user_id"),
        "created_at": r.f("created_at"), "updated_at": r.f("updated_at"), "message_count": r.i("message_count"), "file_count": r.i("file_count"),
        "unread": r.b("unread"),
    })).collect();
    Ok(Json(json!({ "reports": reports })))
}

pub async fn unread(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let admin = auth::is_admin(&u);
    let own = if admin { "" } else { "AND r.user_id=?" };
    let params = if admin { p![] } else { p![u.i("id")] };
    let r = st.db.one(&format!("SELECT COUNT(*) n FROM bug_reports r WHERE {} {own}", unread_sql(admin)), params)?;
    Ok(Json(json!({ "count": r.map(|r| r.i("n")).unwrap_or(0) })))
}

pub async fn detail(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let r = load(&st, id, &u)?;
    let t = now();
    if auth::is_admin(&u) {
        st.db.execute("UPDATE bug_reports SET admin_seen_at=? WHERE id=?", p![t, id])?;
    }
    if r.i("user_id") == u.i("id") {
        st.db.execute("UPDATE bug_reports SET user_seen_at=? WHERE id=?", p![t, id])?;
    }
    Ok(Json(report_json(&load_full(&st, r)?)))
}

pub async fn reply(State(st): State<AppState>, User(u): User, Path(id): Path<i64>, form: Multipart) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let r = load(&st, id, &u)?;
    let admin = auth::is_admin(&u);
    if !admin && st.limits.limited(&format!("bugreply:{}", u.i("id")), REPLIES_PER_HOUR, 3600.0, true) {
        return Err(err(429, "Too many replies in a short time – try again later"));
    }
    let form = read_form(form).await?;
    let body = form.text("body").trim().to_string();
    if body.is_empty() && form.files.is_empty() {
        return Err(err(400, "Write a reply or attach a file"));
    }
    if body.chars().count() > MAX_BODY {
        return Err(err(400, format!("A reply can have at most {MAX_BODY} characters")));
    }
    check_quota(&st, &u, &form.files)?;
    let t = now();
    let mid = st.db.execute(
        "INSERT INTO bug_report_messages (report_id, user_id, username, is_admin, body, created_at) VALUES (?,?,?,?,?,?)",
        p![id, u.i("id"), u.s("username"), admin as i64, body, t],
    )?;
    store_files(&st, id, Some(mid), form.files).await?;
    let status = r.s("status");
    let title = r.s("title");
    if admin {
        let status = if status == "new" { "answered".to_string() } else { status };
        st.db.execute("UPDATE bug_reports SET last_reply_by='admin', status=?, updated_at=?, admin_seen_at=? WHERE id=?", p![status, t, t, id])?;
        // the reporter hears about it by mail, when mail is set up and they have an address (not for a reply
        // to one's own report)
        if email::enabled(&st) && r.i("user_id") != u.i("id") {
            let to = st.db.one("SELECT email FROM users WHERE id=?", p![r.i("user_id")])?.map(|x| x.s("email")).unwrap_or_default();
            if !to.trim().is_empty() {
                let base = st.cfg.str("base_url");
                let link = if base.trim().is_empty() { format!("#/bug-reports/{id}") } else { format!("{}/#/bug-reports/{id}", base.trim().trim_end_matches('/')) };
                let subject = format!("Encore: a reply to your report “{title}”");
                let text = format!("{} answered your report “{title}”:\n\n{body}\n\n{link}\n", u.s("username"));
                let st2 = st.clone();
                tokio::spawn(async move {
                    if let Err(e) = email::send(&st2, &to, &subject, &text).await {
                        st2.db.log("warning", &format!("Bug report reply mail failed: {e}"));
                    }
                });
            }
        }
    } else {
        let status = if status == "answered" { "new".to_string() } else { status };
        st.db.execute("UPDATE bug_reports SET last_reply_by='user', status=?, updated_at=?, user_seen_at=? WHERE id=?", p![status, t, t, id])?;
        let (st2, who) = (st.clone(), u.s("username"));
        let text = if body.is_empty() { format!("{who} on #{id} {title}: (a file)") } else { format!("{who} on #{id} {title}: {}", cut(&body, 300)) };
        tokio::spawn(async move { notify::send(&st2, "Bug report reply", &text, None).await });
    }
    Ok(Json(json!({ "ok": true, "id": mid })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct StatusBody {
    status: String,
}

pub async fn set_status(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<i64>, Json(b): Json<StatusBody>) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let status = b.status.trim();
    if !STATUSES.contains(&status) {
        return Err(err(400, "Pick a status: New, Looking into it, Fixed, Won't fix or Answered"));
    }
    let r = load(&st, id, &u)?;
    st.db.execute("UPDATE bug_reports SET status=?, updated_at=? WHERE id=?", p![status, now(), id])?;
    st.db.log("info", &format!("{} set bug report #{id} ({}) to {}", u.s("username"), r.s("title"), status_label(status)));
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    ensure_table(&st)?;
    let r = load(&st, id, &u)?;
    st.db.execute("DELETE FROM bug_report_files WHERE report_id=?", p![id])?;
    st.db.execute("DELETE FROM bug_report_messages WHERE report_id=?", p![id])?;
    st.db.execute("DELETE FROM bug_reports WHERE id=?", p![id])?;
    let _ = tokio::fs::remove_dir_all(reports_dir(&st).join(id.to_string())).await;
    st.db.log("info", &format!("{} deleted bug report #{id} ({})", u.s("username"), r.s("title")));
    Ok(Json(json!({ "ok": true })))
}

/// `filename="…"; filename*=UTF-8''…` for a Content-Disposition.
fn disposition(inline: bool, name: &str) -> HeaderValue {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let v = format!("{}; filename=\"{ascii}\"; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, urlencoding::encode(name));
    HeaderValue::from_str(&v).unwrap_or(HeaderValue::from_static("attachment"))
}

pub async fn file(State(st): State<AppState>, User(u): User, Path((id, fid)): Path<(i64, i64)>) -> ApiResult<Response> {
    ensure_table(&st)?;
    load(&st, id, &u)?;
    let f = st.db.one("SELECT name, mime, path FROM bug_report_files WHERE id=? AND report_id=?", p![fid, id])?.ok_or_else(|| err(404, "File not found"))?;
    let rel = f.s("path");
    if rel.is_empty() {
        return Err(err(404, "File not found"));
    }
    let data = tokio::fs::read(reports_dir(&st).join(&rel)).await.map_err(|_| err(404, "File not found"))?;
    let mime = f.s("mime");
    let inline = mime.starts_with("image/");
    let mut resp = Body::from(data).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CONTENT_DISPOSITION, disposition(inline, &f.s("name")));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(resp)
}

// ---- export ------------------------------------------------------------------------------------

/// The path of a file inside the zip.
fn zip_path(report_id: i64, f: &Row) -> String {
    format!("files/{report_id}/{}-{}", f.i("id"), f.s("name"))
}

/// A Markdown image or link to the file in the zip. The destination sits in angle brackets: a name like
/// "Screenshot 2026-10-18 (1).png" has spaces and a parenthesis, which end a bare destination.
fn md_file(report_id: i64, f: &Row) -> String {
    let p = zip_path(report_id, f);
    if f.s("mime").starts_with("image/") { format!("![{}](<{p}>)", f.s("name")) } else { format!("[{}](<{p}>)", f.s("name")) }
}

/// reports.md: every report, its meta line, body, files and thread.
fn render_md(reports: &[Full], exported: &str) -> String {
    let mut out = format!("# Encore bug reports ({}, exported {exported})\n", crate::VERSION);
    for full in reports {
        let r = &full.report;
        let id = r.i("id");
        out.push_str(&format!("\n## #{id} [{}] {} — {}\n", kind_label(&r.s("kind")), r.s("title"), status_label(&r.s("status"))));
        let mut meta = vec![format!("by {}", r.s("username")), when(r.f("created_at"))];
        for (label, key) in [("page", "page"), ("app", "app_version"), ("device", "device")] {
            let v = r.s(key);
            if !v.trim().is_empty() {
                meta.push(format!("{label} {}", v.trim()));
            }
        }
        out.push_str(&format!("_{}_\n\n{}\n", meta.join(", "), r.s("body").trim()));
        for f in &full.files {
            out.push_str(&format!("\n{}\n", md_file(id, f)));
        }
        for (m, files) in &full.messages {
            let who = if m.b("is_admin") { format!("{} (admin)", m.s("username")) } else { m.s("username") };
            out.push_str(&format!("\n**{who} — {}**\n\n{}\n", when(m.f("created_at")), m.s("body").trim()));
            for f in files {
                out.push_str(&format!("\n{}\n", md_file(id, f)));
            }
        }
    }
    out
}

/// reports.json: the same shapes as the detail endpoint, with each file's `path` inside the zip.
fn render_json(reports: &[Full], exported: &str) -> Value {
    let file = |rid: i64, f: &Row| json!({ "id": f.i("id"), "name": f.s("name"), "mime": f.s("mime"), "size": f.i("size"), "path": zip_path(rid, f) });
    let list: Vec<Value> = reports.iter().map(|full| {
        let r = &full.report;
        let id = r.i("id");
        json!({
            "id": id, "kind": r.s("kind"), "kind_label": kind_label(&r.s("kind")), "title": r.s("title"), "body": r.s("body"), "page": r.s("page"),
            "app_version": r.s("app_version"), "device": r.s("device"), "status": r.s("status"), "status_label": status_label(&r.s("status")),
            "username": r.s("username"), "user_id": r.i("user_id"), "created_at": r.f("created_at"), "updated_at": r.f("updated_at"),
            "files": full.files.iter().map(|f| file(id, f)).collect::<Vec<_>>(),
            "messages": full.messages.iter().map(|(m, files)| json!({
                "id": m.i("id"), "username": m.s("username"), "is_admin": m.b("is_admin"), "body": m.s("body"), "created_at": m.f("created_at"),
                "files": files.iter().map(|f| file(id, f)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }).collect();
    json!({ "version": crate::VERSION, "exported": exported, "reports": list })
}

pub async fn export(State(st): State<AppState>, Admin(u): Admin, Query(q): Query<ListQ>) -> ApiResult<Response> {
    ensure_table(&st)?;
    let (where_, params) = filter_sql(&q, &u, true);
    // the zip is built in memory: the files it would hold have to fit
    let held = st.db.one(&format!("SELECT COALESCE(SUM(f.size),0) n FROM bug_report_files f JOIN bug_reports r ON r.id=f.report_id WHERE {where_}"), params.clone())?
        .map(|r| r.i("n")).unwrap_or(0);
    if held > MAX_EXPORT_BYTES {
        return Err(err(400, format!("That export would be over {} MB – narrow it with the filters or export reports one by one", MAX_EXPORT_BYTES >> 20)));
    }
    let rows = st.db.query(&format!("SELECT * FROM bug_reports r WHERE {where_} ORDER BY r.id"), params)?;
    let reports: Vec<Full> = rows.into_iter().map(|r| load_full(&st, r)).collect::<ApiResult<_>>()?;
    let exported = ymd(now());
    let base = reports_dir(&st);
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let text = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        z.start_file("reports.md", text)?;
        z.write_all(render_md(&reports, &exported).as_bytes())?;
        z.start_file("reports.json", text)?;
        z.write_all(serde_json::to_string_pretty(&render_json(&reports, &exported))?.as_bytes())?;
        for full in &reports {
            let id = full.report.i("id");
            let files = full.files.iter().chain(full.messages.iter().flat_map(|(_, f)| f.iter()));
            for f in files {
                let rel = f.s("path");
                if rel.is_empty() {
                    continue;
                }
                // a missing file is left out rather than failing the whole export
                let Ok(data) = std::fs::read(base.join(&rel)) else { continue };
                z.start_file(zip_path(id, f), if f.s("mime").starts_with("image/") { stored } else { text })?;
                z.write_all(&data)?;
            }
        }
        Ok(z.finish()?.into_inner())
    })
    .await
    .map_err(|e| err(500, e.to_string()))?
    .map_err(|e| err(500, format!("Could not build the zip: {e}")))?;
    let name = format!("encore-bug-reports-{}.zip", ymd(now()));
    let mut resp = Body::from(bytes).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap_or(HeaderValue::from_static("attachment")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[cfg(test)]
#[path = "bugreports_tests.rs"]
mod tests;
