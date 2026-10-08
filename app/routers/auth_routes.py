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
    return {**user, "requests_this_week": used}


class Prefs(BaseModel):
    localize: bool | None = None
    lang: str | None = None
    latin: bool | None = None


@router.get("/prefs")
def get_prefs(user=Depends(auth.current_user)):
    from ..services import localize
    return {**localize.prefs(user), "languages": localize.LANGS, "job": localize.state}


@router.put("/prefs")
async def put_prefs(body: Prefs, user=Depends(auth.current_user)):
    """Per-user display preferences (localized names). Nothing in the files changes."""
    import asyncio
    import json

    from ..services import localize
    p = localize.prefs(user)
    if body.lang is not None:
        if body.lang not in localize.LANGS:
            raise HTTPException(400, "Unsupported language")
        p["lang"] = body.lang
    for k in ("localize", "latin"):
        if getattr(body, k) is not None:
            p[k] = getattr(body, k)
    db.execute("UPDATE users SET prefs=? WHERE id=?", (json.dumps(p), user["id"]))
    if p["localize"]:
        asyncio.create_task(localize.localize_library(p["lang"], p["latin"]))
    return p


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
