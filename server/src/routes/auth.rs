//! /api/auth: sign in (password + optional authenticator code, or an emailed link), sign up, sessions,
//! password reset, 2FA, profile picture and personal preferences.
use crate::auth::{self, ClientInfo, MaybeUser, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use crate::{email, p};
use axum::extract::{Multipart, Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use totp_rs::{Algorithm, Secret, TOTP};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/info", get(info))
        .route("/api/auth/invite/{code}", get(check_invite))
        .route("/api/auth/login", post(login))
        .route("/api/auth/signup", post(signup))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/logout-all", post(logout_all))
        .route("/api/auth/me", get(me))
        .route("/api/auth/profile", put(profile))
        .route("/api/auth/prefs", get(get_prefs).put(put_prefs))
        .route("/api/auth/password", post(change_password))
        .route("/api/auth/sessions", get(sessions))
        .route("/api/auth/sessions/{sid}", delete(revoke_session))
        .route("/api/auth/forgot", post(forgot))
        .route("/api/auth/reset", post(reset))
        .route("/api/auth/email-link", post(email_link))
        .route("/api/auth/magic", post(magic))
        .route("/api/auth/2fa/setup", post(totp_setup))
        .route("/api/auth/2fa/enable", post(totp_enable))
        .route("/api/auth/2fa/disable", post(totp_disable))
        .route("/api/auth/avatar", post(upload_avatar).delete(delete_avatar))
        .route("/api/users/{id}/avatar", get(avatar))
}

fn session_cookie(token: String, max_age: i64, https: bool) -> Cookie<'static> {
    Cookie::build((auth::COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(https)
        .max_age(time_dur(max_age))
        .build()
}

fn time_dur(secs: i64) -> cookie::time::Duration {
    cookie::time::Duration::seconds(secs)
}

async fn info(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "app_name": st.cfg.str("app_name"),
        "signup_mode": st.cfg.str("signup_mode"),
        "version": env!("CARGO_PKG_VERSION"),
        "build": std::env::var("ENCORE_BUILD").unwrap_or_else(|_| "dev".into()).chars().take(7).collect::<String>(),
        "server": "rust",
        // the web app turns features on only when the server has them
        "features": ["owner", "totp", "email_login", "logout_all", "avatar", "theme", "remote", "rename", "reveal"],
        "email_login": email::enabled(&st) && st.cfg.bool("email_login"),
    }))
}

fn valid_invite(st: &AppState, code: &str) -> ApiResult<crate::db::Row> {
    let inv = st.db.one("SELECT * FROM invites WHERE code=?", p![code.trim()])?;
    match inv {
        Some(i) if i.i("uses") < i.i("max_uses") && (i.get("expires_at").map(|v| v.is_null()).unwrap_or(true) || i.f("expires_at") > now()) => Ok(i),
        _ => Err(err(400, "Invite code is invalid or expired")),
    }
}

async fn check_invite(State(st): State<AppState>, Path(code): Path<String>) -> ApiResult<Json<Value>> {
    let inv = valid_invite(&st, &code)?;
    Ok(Json(json!({ "valid": true, "role": inv.s("role") })))
}

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
    code: Option<String>,
}

fn totp_for(secret_b32: &str, account: &str, issuer: &str) -> Option<TOTP> {
    let bytes = Secret::Encoded(secret_b32.to_string()).to_bytes().ok()?;
    TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes, Some(issuer.to_string()), account.to_string()).ok()
}

fn start_session(st: &AppState, jar: CookieJar, c: &ClientInfo, uid: i64) -> ApiResult<(CookieJar, Json<Value>)> {
    let (token, max_age) = auth::create_session(st, uid, &c.ua, &c.ip)?;
    let u = st.db.one("SELECT * FROM users WHERE id=?", p![uid])?.ok_or_else(|| err(404, "User not found"))?;
    Ok((jar.add(session_cookie(token, max_age, c.https)), Json(auth::public_user(st, &u))))
}

async fn login(State(st): State<AppState>, c: ClientInfo, jar: CookieJar, Json(b): Json<Login>) -> ApiResult<(CookieJar, Json<Value>)> {
    let key = format!("login:{}", c.ip);
    if st.limits.limited(&key, 10, 300.0, false) {
        return Err(err(429, "Too many attempts – wait a few minutes"));
    }
    let user = st.db.one("SELECT * FROM users WHERE username=? COLLATE NOCASE", p![b.username.trim()])?;
    let Some(user) = user.filter(|u| auth::verify_password(&b.password, &u.s("password_hash"))) else {
        st.limits.record(&key);
        st.db.log("warning", &format!("Failed login for '{}' from {}", b.username.chars().take(32).collect::<String>(), c.ip));
        return Err(err(401, "Invalid username or password"));
    };
    if user.b("disabled") {
        return Err(err(403, "This account is disabled"));
    }
    if user.b("totp_enabled") {
        let code = b.code.unwrap_or_default().replace(' ', "");
        if code.is_empty() {
            return Err(err(401, "Enter the 6-digit code from your authenticator app").with(json!({ "two_factor": true })));
        }
        let ok = totp_for(&user.s("totp_secret"), &user.s("username"), &st.cfg.str("app_name")).and_then(|t| t.check_current(&code).ok()).unwrap_or(false);
        if !ok {
            st.limits.record(&key);
            return Err(err(401, "That code didn't match – try the newest one").with(json!({ "two_factor": true })));
        }
    }
    start_session(&st, jar, &c, user.i("id"))
}

#[derive(Deserialize)]
struct Signup {
    username: String,
    password: String,
    email: Option<String>,
    invite: Option<String>,
}

async fn signup(State(st): State<AppState>, c: ClientInfo, jar: CookieJar, Json(b): Json<Signup>) -> ApiResult<(CookieJar, Json<Value>)> {
    let mode = st.cfg.str("signup_mode");
    if st.limits.limited(&format!("signup:{}", c.ip), 5, 3600.0, true) {
        return Err(err(429, "Too many sign-ups from this address"));
    }
    if mode == "closed" {
        return Err(err(403, "Sign-ups are closed"));
    }
    let invite = b.invite.as_deref().filter(|s| !s.trim().is_empty());
    let inv = if mode == "invite" || invite.is_some() {
        let Some(code) = invite else { return Err(err(403, "An invite code is required")) };
        Some(valid_invite(&st, code)?)
    } else {
        None
    };
    // invites can never create an owner
    let role = inv.as_ref().map(|i| i.s("role")).filter(|r| r == "admin").unwrap_or_else(|| "user".into());
    let email = b.email.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let uid = auth::create_user(&st, b.username.trim(), &b.password, &role, email, inv.as_ref().map(|i| i.i("id")))?;
    if let Some(i) = &inv {
        st.db.execute("UPDATE invites SET uses=uses+1 WHERE id=?", p![i.i("id")])?;
    }
    let limit = st.cfg.int("default_request_limit");
    if limit > 0 {
        st.db.execute("UPDATE users SET request_limit=? WHERE id=?", p![limit, uid])?;
    }
    st.db.log("info", &format!("New user signed up: {}", b.username.trim()));
    start_session(&st, jar, &c, uid)
}

async fn logout(State(st): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, Json<Value>)> {
    if let Some(t) = jar.get(auth::COOKIE) {
        st.db.execute("DELETE FROM sessions WHERE token=?", p![t.value()])?;
    }
    Ok((jar.remove(Cookie::build(auth::COOKIE).path("/")), Json(json!({ "ok": true }))))
}

/// Sign out on every device, this one included.
async fn logout_all(State(st): State<AppState>, User(u): User, jar: CookieJar) -> ApiResult<(CookieJar, Json<Value>)> {
    st.db.execute("DELETE FROM sessions WHERE user_id=?", p![u.i("id")])?;
    st.db.log("info", &format!("{} signed out everywhere", u.s("username")));
    Ok((jar.remove(Cookie::build(auth::COOKIE).path("/")), Json(json!({ "ok": true }))))
}

async fn me(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let used = st.db.one(
        "SELECT COUNT(*) n FROM requests WHERE user_id=? AND created_at > ? AND parent_id IS NULL",
        p![u.i("id"), now() - 7.0 * 86400.0],
    )?.map(|r| r.i("n")).unwrap_or(0);
    let mut v = auth::public_user(&st, &u);
    v["requests_this_week"] = json!(used);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct Profile {
    email: Option<String>,
    username: Option<String>,
}

/// Your email address (for password-reset links) and/or a new username.
async fn profile(State(st): State<AppState>, User(u): User, Json(b): Json<Profile>) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    let mut out = json!({ "email": u.s("email"), "username": u.s("username") });
    if let Some(name) = b.username.map(|n| n.trim().to_string()).filter(|n| *n != u.s("username")) {
        auth::validate_username(&name)?;
        if st.db.one("SELECT id FROM users WHERE lower(username)=lower(?) AND id!=?", p![name, uid])?.is_some() {
            return Err(err(409, "That username is taken"));
        }
        st.db.execute("UPDATE users SET username=? WHERE id=?", p![name, uid])?;
        st.db.log("info", &format!("{} is now called {name}", u.s("username")));
        out["username"] = json!(name);
    }
    if let Some(em) = b.email.map(|e| e.trim().to_string()) {
        if !em.is_empty() && (!em.contains('@') || em.len() > 200 || em.contains(' ')) {
            return Err(err(400, "That doesn't look like an email address"));
        }
        st.db.execute("UPDATE users SET email=? WHERE id=?", p![if em.is_empty() { None } else { Some(em.clone()) }, uid])?;
        out["email"] = json!(em);
    }
    Ok(Json(out))
}

/// Personal preferences (appearance etc.). Stored as JSON; unknown keys are kept.
async fn get_prefs(User(u): User) -> Json<Value> {
    Json(serde_json::from_str(&u.s("prefs")).unwrap_or(json!({})))
}

async fn put_prefs(State(st): State<AppState>, User(u): User, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    let mut prefs: Value = serde_json::from_str(&u.s("prefs")).unwrap_or(json!({}));
    if let (Some(p), Some(n)) = (prefs.as_object_mut(), b.as_object()) {
        for (k, v) in n {
            if v.is_null() { p.remove(k); } else { p.insert(k.clone(), v.clone()); }
        }
    }
    let s = prefs.to_string();
    if s.len() > 20_000 {
        return Err(err(400, "Preferences too large"));
    }
    st.db.execute("UPDATE users SET prefs=? WHERE id=?", p![s, u.i("id")])?;
    Ok(Json(prefs))
}

#[derive(Deserialize)]
struct PasswordChange {
    current: String,
    new: String,
}

async fn change_password(State(st): State<AppState>, User(u): User, jar: CookieJar, Json(b): Json<PasswordChange>) -> ApiResult<Json<Value>> {
    let full = st.db.one("SELECT * FROM users WHERE id=?", p![u.i("id")])?.ok_or_else(|| err(404, "User not found"))?;
    if !auth::verify_password(&b.current, &full.s("password_hash")) {
        return Err(err(400, "Current password is incorrect"));
    }
    auth::validate_credentials(&full.s("username"), &b.new)?;
    st.db.execute("UPDATE users SET password_hash=? WHERE id=?", p![auth::hash_password(&b.new), u.i("id")])?;
    let cur = jar.get(auth::COOKIE).map(|c| c.value().to_string()).unwrap_or_default();
    st.db.execute("DELETE FROM sessions WHERE user_id=? AND token != ?", p![u.i("id"), cur])?;
    Ok(Json(json!({ "ok": true })))
}

async fn sessions(State(st): State<AppState>, User(u): User, jar: CookieJar) -> ApiResult<Json<Value>> {
    let cur = jar.get(auth::COOKIE).map(|c| c.value().to_string()).unwrap_or_default();
    let rows = st.db.query("SELECT token, created_at, expires_at, user_agent, ip FROM sessions WHERE user_id=? ORDER BY created_at DESC", p![u.i("id")])?;
    Ok(Json(json!(rows.iter().map(|r| {
        let t = r.s("token");
        json!({ "id": t.chars().take(10).collect::<String>(), "current": t == cur, "created_at": r.get("created_at"), "user_agent": r.get("user_agent"), "ip": r.get("ip"), "device": device_name(&r.s("user_agent")) })
    }).collect::<Vec<_>>())))
}

/// "Chrome on Windows", "Encore app on Android" – instead of a raw user-agent string.
pub fn device_name(ua: &str) -> String {
    let os = if ua.contains("Android") { "Android" } else if ua.contains("iPhone") || ua.contains("iPad") { "iOS" } else if ua.contains("Windows") { "Windows" }
        else if ua.contains("Mac OS") { "macOS" } else if ua.contains("Linux") { "Linux" } else { "" };
    let app = if ua.contains("EncoreAndroid") { "Encore app" } else if ua.contains("Electron") || ua.contains("EncoreDesktop") { "Encore desktop" }
        else if ua.contains("Edg/") { "Edge" } else if ua.contains("Firefox/") { "Firefox" } else if ua.contains("Chrome/") { "Chrome" }
        else if ua.contains("Safari/") { "Safari" } else if ua.is_empty() { "Unknown device" } else { "Browser" };
    if os.is_empty() { app.to_string() } else { format!("{app} on {os}") }
}

async fn revoke_session(State(st): State<AppState>, User(u): User, Path(sid): Path<String>) -> ApiResult<Json<Value>> {
    st.db.execute("DELETE FROM sessions WHERE user_id=? AND substr(token,1,10)=?", p![u.i("id"), sid])?;
    Ok(Json(json!({ "ok": true })))
}

// ---- password reset & sign-in links -----------------------------------------------------------

#[derive(Deserialize)]
struct LoginKey {
    login: String,
}

fn find_by_login(st: &AppState, key: &str) -> ApiResult<Option<crate::db::Row>> {
    Ok(st.db.one(
        "SELECT id, username, email, disabled FROM users WHERE (lower(username)=lower(?) OR lower(email)=lower(?)) AND email IS NOT NULL AND email != ''",
        p![key, key],
    )?)
}

fn base_url(st: &AppState, c: &ClientInfo) -> String {
    let b = st.cfg.str("base_url");
    if b.is_empty() { c.base_url.clone() } else { b.trim_end_matches('/').to_string() }
}

async fn forgot(State(st): State<AppState>, c: ClientInfo, Json(b): Json<LoginKey>) -> ApiResult<Json<Value>> {
    if !email::enabled(&st) {
        return Err(err(400, "Password reset by email isn't set up – ask your admin to reset it"));
    }
    if st.limits.limited(&format!("forgot:{}", c.ip), 5, 900.0, true) {
        return Err(err(429, "Too many attempts – wait a few minutes"));
    }
    if let Some(u) = find_by_login(&st, b.login.trim())? {
        let token = auth::random_token(32);
        st.db.execute("INSERT OR REPLACE INTO password_resets VALUES (?,?,?)", p![u.i("id"), auth::sha256_hex(&token), now() + 3600.0])?;
        let link = format!("{}/#/reset?token={}.{}", base_url(&st, &c), u.i("id"), token);
        let body = format!("Hi {},\n\nOpen this link within an hour to choose a new password:\n{link}\n\nIf you didn't ask for this, ignore this email – your password stays the same.", u.s("username"));
        if let Err(e) = email::send(&st, &u.s("email"), &format!("Reset your {} password", st.cfg.str("app_name")), &body).await {
            st.db.log("error", &format!("Password reset email to {} failed: {e}", u.s("username")));
        }
    }
    Ok(Json(json!({ "ok": true, "message": "If that account has an email address, a reset link is on its way." })))
}

#[derive(Deserialize)]
struct ResetBody {
    token: String,
    password: String,
}

async fn reset(State(st): State<AppState>, Json(b): Json<ResetBody>) -> ApiResult<Json<Value>> {
    let bad = || err(400, "This reset link is invalid or expired – request a new one");
    let (uid, token) = b.token.split_once('.').ok_or_else(bad)?;
    let uid: i64 = uid.parse().map_err(|_| bad())?;
    let saved = st.db.one("SELECT digest, expires_at FROM password_resets WHERE user_id=?", p![uid])?.ok_or_else(bad)?;
    if saved.f("expires_at") < now() || !auth::ct_eq(&saved.s("digest"), &auth::sha256_hex(token)) {
        return Err(bad());
    }
    let u = st.db.one("SELECT id, username FROM users WHERE id=?", p![uid])?.ok_or_else(bad)?;
    auth::validate_credentials(&u.s("username"), &b.password)?;
    st.db.execute("UPDATE users SET password_hash=? WHERE id=?", p![auth::hash_password(&b.password), uid])?;
    st.db.execute("DELETE FROM sessions WHERE user_id=?", p![uid])?;
    st.db.execute("DELETE FROM password_resets WHERE user_id=?", p![uid])?;
    st.db.log("info", &format!("{} reset their password by email", u.s("username")));
    Ok(Json(json!({ "ok": true })))
}

/// Passwordless sign-in: email a one-time link (15 minutes). Accounts with an authenticator app still
/// need their code, so the link is a replacement for the password only.
async fn email_link(State(st): State<AppState>, c: ClientInfo, Json(b): Json<LoginKey>) -> ApiResult<Json<Value>> {
    if !email::enabled(&st) || !st.cfg.bool("email_login") {
        return Err(err(400, "Signing in by email isn't turned on"));
    }
    if st.limits.limited(&format!("emaillink:{}", c.ip), 5, 900.0, true) {
        return Err(err(429, "Too many attempts – wait a few minutes"));
    }
    if let Some(u) = find_by_login(&st, b.login.trim())?.filter(|u| !u.b("disabled")) {
        let token = auth::random_token(32);
        st.db.execute("INSERT INTO login_links VALUES (?,?,?)", p![auth::sha256_hex(&token), u.i("id"), now() + 900.0])?;
        let link = format!("{}/#/magic?token={token}", base_url(&st, &c));
        let body = format!("Hi {},\n\nOpen this link within 15 minutes to sign in:\n{link}\n\nIf you didn't ask for this, ignore this email.", u.s("username"));
        if let Err(e) = email::send(&st, &u.s("email"), &format!("Sign in to {}", st.cfg.str("app_name")), &body).await {
            st.db.log("error", &format!("Sign-in email to {} failed: {e}", u.s("username")));
        }
    }
    Ok(Json(json!({ "ok": true, "message": "If that account has an email address, a sign-in link is on its way." })))
}

#[derive(Deserialize)]
struct MagicBody {
    token: String,
    code: Option<String>,
}

async fn magic(State(st): State<AppState>, c: ClientInfo, jar: CookieJar, Json(b): Json<MagicBody>) -> ApiResult<(CookieJar, Json<Value>)> {
    let digest = auth::sha256_hex(&b.token);
    st.db.execute("DELETE FROM login_links WHERE expires_at < ?", p![now()])?;
    let link = st.db.one("SELECT user_id FROM login_links WHERE digest=?", p![digest])?.ok_or_else(|| err(400, "This sign-in link is invalid or expired"))?;
    let u = st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![link.i("user_id")])?.ok_or_else(|| err(400, "This sign-in link is invalid or expired"))?;
    if u.b("totp_enabled") {
        let code = b.code.unwrap_or_default().replace(' ', "");
        let ok = !code.is_empty() && totp_for(&u.s("totp_secret"), &u.s("username"), &st.cfg.str("app_name")).and_then(|t| t.check_current(&code).ok()).unwrap_or(false);
        if !ok {
            return Err(err(401, "Enter the 6-digit code from your authenticator app").with(json!({ "two_factor": true })));
        }
    }
    st.db.execute("DELETE FROM login_links WHERE digest=?", p![digest])?;
    start_session(&st, jar, &c, u.i("id"))
}

// ---- authenticator app (TOTP) -------------------------------------------------------------------

async fn totp_setup(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let full = st.db.one("SELECT * FROM users WHERE id=?", p![u.i("id")])?.ok_or_else(|| err(404, "User not found"))?;
    if full.b("totp_enabled") {
        return Err(err(400, "Two-factor sign-in is already on – turn it off first to set up a new device"));
    }
    let secret = Secret::generate_secret().to_encoded().to_string();
    st.db.execute("UPDATE users SET totp_secret=? WHERE id=?", p![secret, u.i("id")])?;
    let t = totp_for(&secret, &u.s("username"), &st.cfg.str("app_name")).ok_or_else(|| err(500, "Could not create the secret"))?;
    Ok(Json(json!({ "secret": secret, "otpauth_url": t.get_url(), "qr": t.get_qr_base64().ok() })))
}

#[derive(Deserialize)]
struct CodeBody {
    code: Option<String>,
    password: Option<String>,
}

async fn totp_enable(State(st): State<AppState>, User(u): User, Json(b): Json<CodeBody>) -> ApiResult<Json<Value>> {
    let full = st.db.one("SELECT * FROM users WHERE id=?", p![u.i("id")])?.ok_or_else(|| err(404, "User not found"))?;
    let code = b.code.unwrap_or_default().replace(' ', "");
    let ok = totp_for(&full.s("totp_secret"), &u.s("username"), &st.cfg.str("app_name")).and_then(|t| t.check_current(&code).ok()).unwrap_or(false);
    if !ok {
        return Err(err(400, "That code didn't match – check the time on your phone and try the newest code"));
    }
    st.db.execute("UPDATE users SET totp_enabled=1 WHERE id=?", p![u.i("id")])?;
    st.db.log("info", &format!("{} turned on two-factor sign-in", u.s("username")));
    Ok(Json(json!({ "ok": true })))
}

async fn totp_disable(State(st): State<AppState>, User(u): User, Json(b): Json<CodeBody>) -> ApiResult<Json<Value>> {
    let full = st.db.one("SELECT * FROM users WHERE id=?", p![u.i("id")])?.ok_or_else(|| err(404, "User not found"))?;
    if !auth::verify_password(&b.password.unwrap_or_default(), &full.s("password_hash")) {
        return Err(err(400, "Password is incorrect"));
    }
    st.db.execute("UPDATE users SET totp_enabled=0, totp_secret=NULL WHERE id=?", p![u.i("id")])?;
    st.db.log("info", &format!("{} turned off two-factor sign-in", u.s("username")));
    Ok(Json(json!({ "ok": true })))
}

// ---- profile picture ---------------------------------------------------------------------------

fn avatar_path(st: &AppState, id: i64) -> std::path::PathBuf {
    st.data_dir().join("avatars").join(format!("{id}.jpg"))
}

async fn upload_avatar(State(st): State<AppState>, User(u): User, mut form: Multipart) -> ApiResult<Json<Value>> {
    let mut data = Vec::new();
    while let Some(f) = form.next_field().await.map_err(|e| err(400, e.to_string()))? {
        data = f.bytes().await.map_err(|e| err(400, e.to_string()))?.to_vec();
        if !data.is_empty() {
            break;
        }
    }
    if data.is_empty() || data.len() > 10 * 1024 * 1024 {
        return Err(err(400, "Pick a JPEG, PNG or WebP picture under 10 MB"));
    }
    let path = avatar_path(&st, u.i("id"));
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let img = image::load_from_memory(&data)?;
        let side = img.width().min(img.height());
        let img = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side).resize(256, 256, image::imageops::FilterType::Lanczos3);
        std::fs::create_dir_all(path.parent().unwrap())?;
        img.to_rgb8().save_with_format(&path, image::ImageFormat::Jpeg)?;
        Ok(())
    })
    .await
    .map_err(|e| err(500, e.to_string()))?
    .map_err(|_| err(400, "That file isn't a picture we can read"))?;
    let t = now();
    st.db.execute("UPDATE users SET avatar_at=? WHERE id=?", p![t, u.i("id")])?;
    Ok(Json(json!({ "ok": true, "avatar_at": t })))
}

async fn delete_avatar(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let _ = std::fs::remove_file(avatar_path(&st, u.i("id")));
    st.db.execute("UPDATE users SET avatar_at=NULL WHERE id=?", p![u.i("id")])?;
    Ok(Json(json!({ "ok": true })))
}

async fn avatar(State(st): State<AppState>, MaybeUser(viewer): MaybeUser, Path(id): Path<i64>) -> ApiResult<Response> {
    if viewer.is_none() {
        return Err(err(401, "Not authenticated"));
    }
    let data = tokio::fs::read(avatar_path(&st, id)).await.map_err(|_| err(404, "No picture"))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=86400")], data).into_response())
}
