"""Users, sessions, invites, and FastAPI auth dependencies."""
import os
import secrets
import time
from collections import defaultdict, deque

import bcrypt
from fastapi import Depends, HTTPException, Request

from . import config, db

COOKIE = "encore_session"
_attempts: dict[str, deque] = defaultdict(deque)


def hash_password(pw: str) -> str:
    return bcrypt.hashpw(pw.encode()[:72], bcrypt.gensalt()).decode()


def verify_password(pw: str, hashed: str) -> bool:
    try:
        return bcrypt.checkpw(pw.encode()[:72], hashed.encode())
    except ValueError:
        return False


def rate_limited(key: str, limit: int = 10, window: int = 300, record: bool = True) -> bool:
    """True if `key` exceeded `limit` events in `window` seconds. Records an event unless record=False."""
    q = _attempts[key]
    now = time.time()
    while q and q[0] < now - window:
        q.popleft()
    if len(q) >= limit:
        return True
    if record:
        q.append(now)
    return False


def record_attempt(key: str) -> None:
    _attempts[key].append(time.time())


def validate_credentials(username: str, password: str) -> None:
    if not username or len(username) < 3 or len(username) > 32 or not username.replace("_", "").replace("-", "").replace(".", "").isalnum():
        raise HTTPException(400, "Username must be 3-32 characters: letters, numbers, . _ -")
    if not password or len(password) < 8:
        raise HTTPException(400, "Password must be at least 8 characters")


def create_user(username, password, role="user", email=None, invite_id=None) -> int:
    validate_credentials(username, password)
    if db.one("SELECT id FROM users WHERE username=?", (username,)):
        raise HTTPException(409, "Username already taken")
    return db.execute(
        "INSERT INTO users (username, email, password_hash, role, invite_id, auto_approve, created_at) VALUES (?,?,?,?,?,?,?)",
        (username, email, hash_password(password), role, invite_id, 1 if role == "admin" else 0, time.time()),
    )


def create_session(user_id: int, request: Request) -> tuple[str, int]:
    token = secrets.token_urlsafe(32)
    max_age = int(config.get("session_days")) * 86400
    db.execute(
        "INSERT INTO sessions VALUES (?,?,?,?,?,?)",
        (token, user_id, time.time(), time.time() + max_age, request.headers.get("user-agent", "")[:200], client_ip(request)),
    )
    db.execute("UPDATE users SET last_login=? WHERE id=?", (time.time(), user_id))
    return token, max_age


def client_ip(request: Request) -> str:
    fwd = request.headers.get("x-forwarded-for")
    if fwd:
        return fwd.split(",")[0].strip()
    return request.client.host if request.client else ""


def ensure_admin() -> None:
    if db.one("SELECT id FROM users WHERE role='admin'"):
        return
    username = os.environ.get("ENCORE_ADMIN_USER", "admin")
    password = os.environ.get("ENCORE_ADMIN_PASSWORD")
    generated = not password
    if generated:
        password = secrets.token_urlsafe(12)
    create_user(username, password, role="admin")
    msg = f"Created admin user '{username}'"
    if generated:
        msg += f" with generated password: {password}  (change it in Settings → Account)"
        pw_file = os.path.join(db.DATA_DIR, "initial_admin_password.txt")
        with open(pw_file, "w") as fh:
            fh.write(f"{username}\n{password}\n")
        os.chmod(pw_file, 0o600)
    print("=" * 70, f"\n{msg}\n", "=" * 70, flush=True)
    db.log("info", f"Created admin user '{username}'")


def _user_from_request(request: Request) -> dict | None:
    api_key = request.headers.get("x-api-key")
    if api_key and config.get("api_key") and secrets.compare_digest(api_key, config.get("api_key")):
        return db.one("SELECT * FROM users WHERE role='admin' AND disabled=0 ORDER BY id LIMIT 1")
    token = request.cookies.get(COOKIE)
    if not token:
        return None
    s = db.one("SELECT * FROM sessions WHERE token=?", (token,))
    if not s or s["expires_at"] < time.time():
        return None
    user = db.one("SELECT * FROM users WHERE id=? AND disabled=0", (s["user_id"],))
    return user


def current_user(request: Request) -> dict:
    user = _user_from_request(request)
    if not user:
        raise HTTPException(401, "Not authenticated")
    # CSRF: mutating cookie-authenticated requests must come from our own JS
    if request.method not in ("GET", "HEAD", "OPTIONS") and not request.headers.get("x-api-key"):
        if request.headers.get("x-requested-with") != "encore":
            raise HTTPException(403, "Missing CSRF header")
    user.pop("password_hash", None)
    return user


def admin_user(user: dict = Depends(current_user)) -> dict:
    if user["role"] != "admin":
        raise HTTPException(403, "Admin only")
    return user


def public_user(u: dict) -> dict:
    return {k: v for k, v in u.items() if k != "password_hash"}
