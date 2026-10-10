import time

from fastapi import APIRouter, Depends, HTTPException, Request, Response
from pydantic import BaseModel

from .. import auth, config, db

router = APIRouter(prefix="/api/auth", tags=["auth"])


class Login(BaseModel):
    username: str
    password: str


class Signup(BaseModel):
    username: str
    password: str
    email: str | None = None
    invite: str | None = None


class PasswordChange(BaseModel):
    current: str
    new: str


def _set_cookie(resp: Response, request: Request, token: str, max_age: int):
    secure = request.url.scheme == "https" or request.headers.get("x-forwarded-proto") == "https"
    resp.set_cookie(auth.COOKIE, token, max_age=max_age, httponly=True, samesite="lax", secure=secure, path="/")


@router.get("/info")
def info():
    import os

    from .. import __version__
    return {"app_name": config.get("app_name"), "signup_mode": config.get("signup_mode"),
            "version": __version__, "build": os.environ.get("ENCORE_BUILD", "dev")[:7]}


@router.get("/invite/{code}")
def check_invite(code: str):
    inv = _valid_invite(code)
    return {"valid": True, "role": inv["role"]}


def _valid_invite(code: str) -> dict:
    inv = db.one("SELECT * FROM invites WHERE code=?", (code.strip(),))
    if not inv or inv["uses"] >= inv["max_uses"] or (inv["expires_at"] and inv["expires_at"] < time.time()):
        raise HTTPException(400, "Invite code is invalid or expired")
    return inv


@router.post("/login")
def login(body: Login, request: Request, response: Response):
    ip = auth.client_ip(request)
    if auth.rate_limited(f"login:{ip}", record=False):
        raise HTTPException(429, "Too many attempts – wait a few minutes")
    user = db.one("SELECT * FROM users WHERE username=?", (body.username.strip(),))
    if not user or not auth.verify_password(body.password, user["password_hash"]):
        auth.record_attempt(f"login:{ip}")
        db.log("warning", f"Failed login for '{body.username[:32]}' from {ip}")
        raise HTTPException(401, "Invalid username or password")
    if user["disabled"]:
        raise HTTPException(403, "This account is disabled")
    token, max_age = auth.create_session(user["id"], request)
    _set_cookie(response, request, token, max_age)
    return auth.public_user(user)


@router.post("/signup")
def signup(body: Signup, request: Request, response: Response):
    mode = config.get("signup_mode")
    if auth.rate_limited(f"signup:{auth.client_ip(request)}", 5, 3600):
        raise HTTPException(429, "Too many sign-ups from this address")
    inv = None
    if mode == "closed":
        raise HTTPException(403, "Sign-ups are closed")
    if mode == "invite" or body.invite:
        if not body.invite:
            raise HTTPException(403, "An invite code is required")
        inv = _valid_invite(body.invite)
    uid = auth.create_user(body.username.strip(), body.password, role=inv["role"] if inv else "user",
                           email=(body.email or "").strip() or None, invite_id=inv["id"] if inv else None)
    if inv:
        db.execute("UPDATE invites SET uses=uses+1 WHERE id=?", (inv["id"],))
    limit = config.get("default_request_limit")
    if limit:
        db.execute("UPDATE users SET request_limit=? WHERE id=?", (limit, uid))
    db.log("info", f"New user signed up: {body.username}" + (f" (invite {inv['code']})" if inv else ""))
    token, max_age = auth.create_session(uid, request)
    _set_cookie(response, request, token, max_age)
    return auth.public_user(db.one("SELECT * FROM users WHERE id=?", (uid,)))


@router.post("/logout")
def logout(request: Request, response: Response):
    token = request.cookies.get(auth.COOKIE)
    if token:
        db.execute("DELETE FROM sessions WHERE token=?", (token,))
    response.delete_cookie(auth.COOKIE, path="/")
    return {"ok": True}


@router.get("/me")
def me(user=Depends(auth.current_user)):
    used = db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND created_at > ? AND parent_id IS NULL", (user["id"], time.time() - 7 * 86400))["n"]
    return {**user, "perms": auth.perms_of(user), "requests_this_week": used}


class Profile(BaseModel):
    email: str = ""


@router.put("/profile")
def update_profile(body: Profile, user=Depends(auth.current_user)):
    """Your email address – used for password-reset links."""
    em = body.email.strip()
    if em and ("@" not in em or len(em) > 200 or " " in em):
        raise HTTPException(400, "That doesn't look like an email address")
    db.execute("UPDATE users SET email=? WHERE id=?", (em or None, user["id"]))
    return {"email": em}


class ForgotBody(BaseModel):
    login: str


class ResetBody(BaseModel):
    token: str
    password: str


_RESET_TTL = 3600


@router.post("/forgot")
async def forgot_password(body: ForgotBody, request: Request):
    """Email a one-hour reset link. Always answers the same, so it can't be used to find accounts."""
    import hashlib
    import secrets as _secrets

    from ..services import email
    if not email.enabled():
        raise HTTPException(400, "Password reset by email isn't set up – ask your admin to reset it")
    if auth.rate_limited(f"forgot:{auth.client_ip(request)}", limit=5, window=900):
        raise HTTPException(429, "Too many attempts – wait a few minutes")
    key = body.login.strip()
    u = db.one("SELECT id, username, email FROM users WHERE (lower(username)=lower(?) OR lower(email)=lower(?)) AND email IS NOT NULL AND email != ''",
               (key, key))
    if u:
        token = _secrets.token_urlsafe(32)
        digest = hashlib.sha256(token.encode()).hexdigest()
        db.execute("INSERT OR REPLACE INTO password_resets VALUES (?,?,?)", (u["id"], digest, time.time() + _RESET_TTL))
        base = (config.get("base_url") or str(request.base_url)).rstrip("/")
        try:
            await email.send(u["email"], f"Reset your {config.get('app_name')} password",
                             f"Hi {u['username']},\n\nOpen this link within an hour to choose a new password:\n{base}/#/reset?token={u['id']}.{token}\n\n"
                             "If you didn't ask for this, ignore this email – your password stays the same.")
        except Exception as e:  # noqa: BLE001
            db.log("error", f"Password reset email to {u['username']} failed: {e}")
    return {"ok": True, "message": "If that account has an email address, a reset link is on its way."}


@router.post("/reset")
def reset_password(body: ResetBody):
    import hashlib
    import hmac
    uid, _, token = body.token.partition(".")
    saved = db.one("SELECT digest, expires_at FROM password_resets WHERE user_id=?", (int(uid),)) if uid.isdigit() else None
    if not saved or saved["expires_at"] < time.time() or not hmac.compare_digest(saved["digest"], hashlib.sha256(token.encode()).hexdigest()):
        raise HTTPException(400, "This reset link is invalid or expired – request a new one")
    u = db.one("SELECT id, username FROM users WHERE id=?", (int(uid),))
    if not u:
        raise HTTPException(400, "This reset link is invalid or expired – request a new one")
    auth.validate_credentials(u["username"], body.password)
    db.execute("UPDATE users SET password_hash=? WHERE id=?", (auth.hash_password(body.password), u["id"]))
    db.execute("DELETE FROM sessions WHERE user_id=?", (u["id"],))
    db.execute("DELETE FROM password_resets WHERE user_id=?", (u["id"],))
    db.log("info", f"{u['username']} reset their password by email")
    return {"ok": True}


@router.post("/password")
def change_password(body: PasswordChange, request: Request, user=Depends(auth.current_user)):
    full = db.one("SELECT * FROM users WHERE id=?", (user["id"],))
    if not auth.verify_password(body.current, full["password_hash"]):
        raise HTTPException(400, "Current password is incorrect")
    auth.validate_credentials(full["username"], body.new)
    db.execute("UPDATE users SET password_hash=? WHERE id=?", (auth.hash_password(body.new), user["id"]))
    db.execute("DELETE FROM sessions WHERE user_id=? AND token != ?", (user["id"], request.cookies.get(auth.COOKIE, "")))
    return {"ok": True}


@router.get("/sessions")
def my_sessions(request: Request, user=Depends(auth.current_user)):
    cur = request.cookies.get(auth.COOKIE)
    rows = db.query("SELECT token, created_at, expires_at, user_agent, ip FROM sessions WHERE user_id=? ORDER BY created_at DESC", (user["id"],))
    return [{"id": r["token"][:10], "current": r["token"] == cur, **{k: r[k] for k in ("created_at", "user_agent", "ip")}} for r in rows]


@router.delete("/sessions/{sid}")
def revoke_session(sid: str, user=Depends(auth.current_user)):
    db.execute("DELETE FROM sessions WHERE user_id=? AND substr(token,1,10)=?", (user["id"], sid))
    return {"ok": True}
