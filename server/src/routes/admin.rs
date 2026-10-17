//! /api/admin: users & permissions, invites, settings, activity log.
use crate::auth::{self, Admin, Owner};
use crate::config::Config;
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::p;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/users", get(users).post(create_user))
        .route("/api/admin/users/bulk", post(bulk))
        .route("/api/admin/users/{uid}", patch(update_user).delete(delete_user))
        .route("/api/admin/permissions", get(permissions))
        .route("/api/admin/invites", get(invites).post(create_invite))
        .route("/api/admin/invites/{iid}", delete(delete_invite))
        .route("/api/admin/settings", get(get_settings).put(put_settings))
        .route("/api/admin/settings/api-key", post(regen_api_key))
        .route("/api/admin/settings/secret/{key}", get(reveal_secret))
        .route("/api/admin/activity", get(activity))
        .route("/api/system/api-key", get(api_key))
        .route("/api/system/activity", get(activity))
}

async fn users(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let rows = st.db.query(
        "SELECT u.id, u.username, u.email, u.role, u.disabled, u.auto_approve, u.request_limit, u.created_at, u.last_login, u.perms,
                u.totp_enabled, u.avatar_at, i.code AS invite_code,
                (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.parent_id IS NULL) AS request_count
         FROM users u LEFT JOIN invites i ON i.id=u.invite_id ORDER BY u.role='owner' DESC, u.created_at",
        p![],
    )?;
    Ok(Json(json!(rows.into_iter().map(|mut r| {
        let perms = auth::perms_of(&st, &r);
        r.insert("perms".into(), Value::Object(perms));
        r.insert("totp_enabled".into(), json!(r.b("totp_enabled")));
        r
    }).collect::<Vec<_>>())))
}

async fn permissions(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let users = st.db.query("SELECT id, role, auto_approve, perms FROM users WHERE role='user'", p![])?;
    let perms: Vec<Value> = auth::PERMS.iter().map(|(k, label)| {
        let on = users.iter().filter(|u| auth::perms_of(&st, u).get(*k).and_then(|v| v.as_bool()).unwrap_or(false)).count();
        json!({ "key": k, "label": label, "on": on, "of": users.len() })
    }).collect();
    Ok(Json(json!({ "perms": perms })))
}

#[derive(Deserialize)]
struct BulkPerm {
    perm: String,
    value: bool,
}

/// One permission on/off for every regular user at once (admins and the owner have everything anyway).
async fn bulk(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<BulkPerm>) -> ApiResult<Json<Value>> {
    if !auth::PERMS.iter().any(|(k, _)| *k == b.perm) {
        return Err(err(400, "Unknown permission"));
    }
    for u in st.db.query("SELECT id, perms FROM users WHERE role='user'", p![])? {
        if b.perm == "auto_approve" {
            st.db.execute("UPDATE users SET auto_approve=? WHERE id=?", p![b.value as i64, u.i("id")])?;
            continue;
        }
        let mut p: Map<String, Value> = serde_json::from_str(&u.s("perms")).unwrap_or_default();
        p.insert(b.perm.clone(), json!(b.value));
        st.db.execute("UPDATE users SET perms=? WHERE id=?", p![Value::Object(p).to_string(), u.i("id")])?;
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct UserCreate {
    username: String,
    password: String,
    email: Option<String>,
    #[serde(default = "user_role")]
    role: String,
}
fn user_role() -> String {
    "user".into()
}

async fn create_user(State(st): State<AppState>, Admin(me): Admin, Json(b): Json<UserCreate>) -> ApiResult<Json<Value>> {
    if !matches!(b.role.as_str(), "user" | "admin") {
        return Err(err(400, "Invalid role"));
    }
    if b.role == "admin" && !auth::is_owner(&me) {
        return Err(err(403, "Only the server owner can create admins"));
    }
    let email = b.email.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let id = auth::create_user(&st, b.username.trim(), &b.password, &b.role, email, None)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct UserUpdate {
    role: Option<String>,
    disabled: Option<bool>,
    auto_approve: Option<bool>,
    request_limit: Option<i64>,
    email: Option<String>,
    password: Option<String>,
    perms: Option<Map<String, Value>>,
}

/// Who may change whom: nobody touches the owner (except the owner, who can't demote/disable themselves);
/// only the owner changes admins or makes someone an admin; admins manage regular users.
fn may_manage(me: &crate::db::Row, target: &crate::db::Row) -> ApiResult<()> {
    if auth::is_owner(target) && me.i("id") != target.i("id") {
        return Err(err(403, "Nobody can change the server owner's account"));
    }
    if target.s("role") == "admin" && !auth::is_owner(me) && me.i("id") != target.i("id") {
        return Err(err(403, "Only the server owner can change other admins"));
    }
    Ok(())
}

async fn update_user(State(st): State<AppState>, Admin(me): Admin, Path(uid): Path<i64>, Json(b): Json<UserUpdate>) -> ApiResult<Json<Value>> {
    let u = st.db.one("SELECT * FROM users WHERE id=?", p![uid])?.ok_or_else(|| err(404, "User not found"))?;
    may_manage(&me, &u)?;
    let me_target = uid == me.i("id");
    let mut sets: Vec<(&str, Value)> = vec![];
    if let Some(role) = &b.role {
        if !matches!(role.as_str(), "user" | "admin") {
            return Err(err(400, "Invalid role"));
        }
        if me_target {
            if role != &u.s("role") {
                return Err(err(400, "You can't change your own role"));
            }
        } else {
            if (role == "admin" || u.s("role") == "admin") && !auth::is_owner(&me) {
                return Err(err(403, "Only the server owner can make or remove admins"));
            }
            sets.push(("role", json!(role)));
        }
    }
    if let Some(d) = b.disabled {
        if me_target && d {
            return Err(err(400, "You can't disable yourself"));
        }
        sets.push(("disabled", json!(d as i64)));
    }
    if let Some(a) = b.auto_approve {
        sets.push(("auto_approve", json!(a as i64)));
    }
    if let Some(l) = b.request_limit {
        sets.push(("request_limit", if l > 0 { json!(l) } else { Value::Null }));
    }
    if let Some(e) = &b.email {
        sets.push(("email", if e.trim().is_empty() { Value::Null } else { json!(e.trim()) }));
    }
    if let Some(np) = &b.perms {
        let mut cur: Map<String, Value> = serde_json::from_str(&u.s("perms")).unwrap_or_default();
        for (k, v) in np {
            if k == "auto_approve" {
                sets.push(("auto_approve", json!(v.as_bool().unwrap_or(false) as i64)));
            } else if auth::PERMS.iter().any(|(p, _)| p == k) {
                cur.insert(k.clone(), json!(v.as_bool().unwrap_or(false)));
            }
        }
        sets.push(("perms", json!(Value::Object(cur).to_string())));
    }
    if let Some(pw) = b.password.as_deref().filter(|s| !s.is_empty()) {
        auth::validate_credentials(&u.s("username"), pw)?;
        sets.push(("password_hash", json!(auth::hash_password(pw))));
    }
    if !sets.is_empty() {
        let cols = sets.iter().map(|(k, _)| format!("{k}=?")).collect::<Vec<_>>().join(", ");
        let mut params: Vec<Value> = sets.into_iter().map(|(_, v)| v).collect();
        params.push(json!(uid));
        st.db.execute(&format!("UPDATE users SET {cols} WHERE id=?"), params)?;
    }
    if b.disabled == Some(true) || b.password.as_deref().is_some_and(|s| !s.is_empty()) {
        st.db.execute("DELETE FROM sessions WHERE user_id=?", p![uid])?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_user(State(st): State<AppState>, Admin(me): Admin, Path(uid): Path<i64>) -> ApiResult<Json<Value>> {
    if uid == me.i("id") {
        return Err(err(400, "You can't delete yourself"));
    }
    let u = st.db.one("SELECT * FROM users WHERE id=?", p![uid])?.ok_or_else(|| err(404, "User not found"))?;
    may_manage(&me, &u)?;
    st.db.execute("DELETE FROM sessions WHERE user_id=?", p![uid])?;
    st.db.execute("DELETE FROM users WHERE id=?", p![uid])?;
    st.db.log("info", &format!("{} deleted user {}", me.s("username"), u.s("username")));
    Ok(Json(json!({ "ok": true })))
}

async fn invites(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.query(
        "SELECT i.*, u.username AS created_by_name,
                (SELECT group_concat(username, ', ') FROM users WHERE invite_id=i.id) AS used_by
         FROM invites i LEFT JOIN users u ON u.id=i.created_by ORDER BY i.created_at DESC",
        p![],
    )?)))
}

#[derive(Deserialize)]
struct InviteCreate {
    #[serde(default = "one")]
    max_uses: i64,
    expires_days: Option<f64>,
    #[serde(default = "user_role")]
    role: String,
    note: Option<String>,
}
fn one() -> i64 {
    1
}

async fn create_invite(State(st): State<AppState>, Admin(me): Admin, Json(b): Json<InviteCreate>) -> ApiResult<Json<Value>> {
    if !matches!(b.role.as_str(), "user" | "admin") {
        return Err(err(400, "Invalid role"));
    }
    if b.role == "admin" && !auth::is_owner(&me) {
        return Err(err(403, "Only the server owner can invite admins"));
    }
    let code = (0..3).map(|_| auth::random_hex(2).to_uppercase()).collect::<Vec<_>>().join("-");
    let expires = b.expires_days.filter(|d| *d > 0.0).map(|d| now() + d * 86400.0);
    let id = st.db.execute(
        "INSERT INTO invites (code, created_by, role, max_uses, expires_at, note, created_at) VALUES (?,?,?,?,?,?,?)",
        p![code, me.i("id"), b.role, b.max_uses.max(1), expires, b.note, now()],
    )?;
    Ok(Json(json!(st.db.one("SELECT * FROM invites WHERE id=?", p![id])?)))
}

async fn delete_invite(State(st): State<AppState>, Admin(_): Admin, Path(iid): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.execute("DELETE FROM invites WHERE id=?", p![iid])?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_settings(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    Json(st.cfg.public_dump())
}

async fn put_settings(State(st): State<AppState>, Admin(me): Admin, Json(values): Json<Map<String, Value>>) -> ApiResult<Json<Value>> {
    let mut clean = Map::new();
    for (k, v) in values {
        if st.cfg.def(&k).is_none() || Config::is_masked(&v) || k == "api_key" || st.cfg.env_locked(&k) {
            continue;
        }
        clean.insert(k, v);
    }
    if let Some(m) = clean.get("signup_mode").and_then(|v| v.as_str()) {
        if !matches!(m, "invite" | "open" | "closed") {
            return Err(err(400, "signup_mode must be invite, open or closed"));
        }
    }
    let mut keys: Vec<&String> = clean.keys().collect();
    keys.sort();
    st.db.log("info", &format!("{} updated settings: {}", me.s("username"), keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    let warnings = folder_warnings(&clean, |k| st.cfg.get(k));
    st.cfg.set_many(&Value::Object(clean));
    crate::slskd_managed::poke(); // a changed Soulseek login or port takes effect now, not at the next check
    let mut out = st.cfg.public_dump();
    if !warnings.is_empty() {
        out["_warnings"] = json!(warnings);
    }
    Ok(Json(out))
}

/// Folders Encore has to write to, probed when one changes so a wrong PUID/PGID shows on the card at
/// once (with the chown line) instead of at the first download: the Soulseek and staging folders, every
/// download client's folder and the blackhole folders. Only the two folders Encore owns (Soulseek
/// downloads, staging) are created by the probe; a download client's or blackhole folder has to be
/// mounted already, like the library – an empty one made inside the container would hide a missing
/// volume mapping. `current` gives a key's saved value: a folder saved unchanged (the rest of its
/// section was edited) is not probed again.
fn folder_warnings(saved: &Map<String, Value>, current: impl Fn(&str) -> Value) -> Vec<Value> {
    let mut out = vec![];
    for (k, v) in saved {
        let folder = k == "slskd_download_path" || k == "staging_path" || k == "library_path"
            || k.ends_with("_download_path") || k.ends_with("_blackhole_watch") || k.ends_with("_blackhole_completed");
        let Some(path) = v.as_str().map(str::trim).filter(|p| folder && !p.is_empty()) else { continue };
        if current(k).as_str().map(str::trim) == Some(path) {
            continue;
        }
        let dir = std::path::Path::new(path);
        let owned = k == "slskd_download_path" || k == "staging_path";
        if let Err(why) = crate::slskd_managed::probe_writable(dir, owned) {
            out.push(json!({ "key": k, "message": crate::slskd_managed::unwritable(dir, &why) }));
        }
    }
    out
}

/// The API key acts as the owner – only the owner may see or change it.
async fn api_key(State(st): State<AppState>, Owner(_): Owner) -> Json<Value> {
    Json(json!({ "api_key": st.cfg.str("api_key") }))
}

/// One saved secret in plain text, for the eye/copy buttons in Settings. Owner only, and logged.
async fn reveal_secret(State(st): State<AppState>, Owner(me): Owner, Path(key): Path<String>) -> ApiResult<Json<Value>> {
    if st.cfg.def(&key).is_none() {
        return Err(err(404, "Unknown setting"));
    }
    // the WireGuard private key never goes back to a browser
    if key == "vpn_wireguard_config" {
        return Err(err(403, "The WireGuard config can't be shown – paste it again to replace it"));
    }
    st.db.log("info", &format!("{} viewed the saved {key}", me.s("username")));
    let v = st.cfg.get(&key);
    Ok(Json(json!({ "key": key, "value": v.as_str().map(String::from).unwrap_or_else(|| v.to_string()) })))
}

async fn regen_api_key(State(st): State<AppState>, Owner(_): Owner) -> Json<Value> {
    let key = auth::random_hex(24);
    st.cfg.set_many(&json!({ "api_key": key }));
    Json(json!({ "api_key": key }))
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<i64>,
}

async fn activity(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<Limit>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.query("SELECT * FROM activity ORDER BY id DESC LIMIT ?", p![q.limit.unwrap_or(300).clamp(1, 1000)])?)))
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    /// Saving a folder Encore can't write to answers with the chown line; the library and a download client's
    /// folder have to exist (never created); a folder saved unchanged is not probed.
    #[test]
    fn saved_folders_are_probed() {
        let dir = std::env::temp_dir().join(format!("encore-admin-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // root may write anywhere: a file where the folder should be stops it too
        std::fs::write(dir.join("file"), "x").unwrap();
        let bad = dir.join("file").join("slskd");
        let mut saved = Map::new();
        saved.insert("slskd_download_path".into(), json!(bad.to_string_lossy()));
        saved.insert("staging_path".into(), json!(dir.join("staging").to_string_lossy()));
        saved.insert("library_path".into(), json!(dir.join("missing").to_string_lossy()));
        saved.insert("soulseek_username".into(), json!("/not/a/folder"));
        // saved unchanged with the rest of its section: not probed (it would fail – a file is in the way)
        let same = dir.join("file").join("watch");
        saved.insert("torrent_blackhole_watch".into(), json!(same.to_string_lossy()));
        // a download client's folder that is not mounted: reported, never created
        let qbit = dir.join("qbit");
        saved.insert("qbit_download_path".into(), json!(qbit.to_string_lossy()));
        let w = folder_warnings(&saved, |k| if k == "torrent_blackhole_watch" { json!(same.to_string_lossy()) } else { Value::Null });
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.iter().all(|x| x["key"] != "torrent_blackhole_watch"), "an unchanged folder is not probed: {w:?}");
        let q = w.iter().find(|x| x["key"] == "qbit_download_path").unwrap()["message"].as_str().unwrap().to_string();
        assert!(q.contains("doesn't exist inside the container"), "{q}");
        assert!(!qbit.exists(), "a download client's folder is never created by a save");
        let slskd = w.iter().find(|x| x["key"] == "slskd_download_path").unwrap()["message"].as_str().unwrap().to_string();
        assert!(slskd.contains(&format!("chown {}:{} {}", unsafe { libc::geteuid() }, unsafe { libc::getegid() }, bad.display())), "{slskd}");
        assert!(slskd.contains("or set PUID/PGID"), "{slskd}");
        let lib = w.iter().find(|x| x["key"] == "library_path").unwrap()["message"].as_str().unwrap().to_string();
        assert!(lib.contains("doesn't exist inside the container"), "{lib}");
        assert!(!dir.join("missing").exists(), "the library folder is never created by a save");
        assert!(dir.join("staging").is_dir(), "a writable download folder is made");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
