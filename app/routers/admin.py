import secrets
import time

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from .. import auth, config, db

router = APIRouter(prefix="/api/admin", tags=["admin"], dependencies=[Depends(auth.admin_user)])


class UserCreate(BaseModel):
    username: str
    password: str
    email: str | None = None
    role: str = "user"


class UserUpdate(BaseModel):
    role: str | None = None
    disabled: bool | None = None
    auto_approve: bool | None = None
    request_limit: int | None = None
    email: str | None = None
    password: str | None = None


class InviteCreate(BaseModel):
    max_uses: int = 1
    expires_days: float | None = 7
    role: str = "user"
    note: str | None = None


@router.get("/users")
def users():
    rows = db.query("""
        SELECT u.id, u.username, u.email, u.role, u.disabled, u.auto_approve, u.request_limit, u.created_at, u.last_login,
               i.code AS invite_code,
               (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.parent_id IS NULL) AS request_count
        FROM users u LEFT JOIN invites i ON i.id=u.invite_id ORDER BY u.created_at""")
    return rows


@router.post("/users")
def create_user(body: UserCreate):
    if body.role not in ("user", "admin"):
        raise HTTPException(400, "Invalid role")
    uid = auth.create_user(body.username.strip(), body.password, body.role, body.email)
    db.log("info", f"Admin created user {body.username}")
    return {"id": uid}


@router.patch("/users/{uid}")
def update_user(uid: int, body: UserUpdate, admin=Depends(auth.admin_user)):
    u = db.one("SELECT * FROM users WHERE id=?", (uid,))
    if not u:
        raise HTTPException(404, "User not found")
    if uid == admin["id"] and (body.disabled or (body.role and body.role != "admin")):
        raise HTTPException(400, "You can't demote or disable yourself")
    fields = {}
    if body.role is not None:
        if body.role not in ("user", "admin"):
            raise HTTPException(400, "Invalid role")
        fields["role"] = body.role
    if body.disabled is not None:
        fields["disabled"] = int(body.disabled)
    if body.auto_approve is not None:
        fields["auto_approve"] = int(body.auto_approve)
    if body.request_limit is not None:
        fields["request_limit"] = body.request_limit or None
    if body.email is not None:
        fields["email"] = body.email or None
    if body.password:
        auth.validate_credentials(u["username"], body.password)
        fields["password_hash"] = auth.hash_password(body.password)
    if fields:
        db.execute(f"UPDATE users SET {', '.join(k + '=?' for k in fields)} WHERE id=?", (*fields.values(), uid))
    if body.disabled or body.password:
        db.execute("DELETE FROM sessions WHERE user_id=?", (uid,))
    return {"ok": True}


@router.delete("/users/{uid}")
def delete_user(uid: int, admin=Depends(auth.admin_user)):
    if uid == admin["id"]:
        raise HTTPException(400, "You can't delete yourself")
    db.execute("DELETE FROM sessions WHERE user_id=?", (uid,))
    db.execute("DELETE FROM users WHERE id=?", (uid,))
    return {"ok": True}


@router.get("/invites")
def invites():
    return db.query("""SELECT i.*, u.username AS created_by_name,
        (SELECT group_concat(username, ', ') FROM users WHERE invite_id=i.id) AS used_by
        FROM invites i LEFT JOIN users u ON u.id=i.created_by ORDER BY i.created_at DESC""")


@router.post("/invites")
def create_invite(body: InviteCreate, admin=Depends(auth.admin_user)):
    if body.role not in ("user", "admin"):
        raise HTTPException(400, "Invalid role")
    code = "-".join(secrets.token_hex(2).upper() for _ in range(3))
    expires = time.time() + body.expires_days * 86400 if body.expires_days else None
    iid = db.execute(
        "INSERT INTO invites (code, created_by, role, max_uses, expires_at, note, created_at) VALUES (?,?,?,?,?,?,?)",
        (code, admin["id"], body.role, max(1, body.max_uses), expires, body.note, time.time()),
    )
    return db.one("SELECT * FROM invites WHERE id=?", (iid,))


@router.delete("/invites/{iid}")
def delete_invite(iid: int):
    db.execute("UPDATE users SET invite_id=NULL WHERE invite_id=?", (iid,))
    db.execute("DELETE FROM invites WHERE id=?", (iid,))
    return {"ok": True}


@router.get("/settings")
def get_settings():
    return config.public_dump()


@router.put("/settings")
def put_settings(values: dict):
    clean = {k: v for k, v in values.items()
             if k in config.DEFAULTS and not config.is_masked(v) and k != "api_key" and not config.env_locked(k)}
    if "signup_mode" in clean and clean["signup_mode"] not in ("invite", "open", "closed"):
        raise HTTPException(400, "signup_mode must be invite, open or closed")
    config.set_many(clean)
    db.log("info", "Settings updated: " + ", ".join(sorted(clean)))
    return config.public_dump()


@router.post("/settings/api-key")
def regen_api_key():
    key = secrets.token_hex(24)
    config.set_many({"api_key": key})
    return {"api_key": key}


@router.get("/activity")
def activity(limit: int = 200):
    return db.query("SELECT * FROM activity ORDER BY id DESC LIMIT ?", (min(limit, 1000),))


# ---- auto-download -----------------------------------------------------------------

AUTO_KEYS = ["auto_request_followed", "auto_request_likes", "auto_request_follows", "follow_check_hours", "album_upgrade", "quality_first", "upgrade_lossy",
             "upgrade_lossy_days", "sync_interval_minutes", "auto_approve", "source_order", "discover_auto_count", "discover_catalog_merge"]


class AutoDownload(BaseModel):
    settings: dict = {}
    users: dict[str, bool] = {}


@router.get("/autodownload")
def get_autodownload():
    stats = db.one("""SELECT
        SUM(CASE WHEN detail LIKE '%"auto": true%' AND status IN ('queued','searching','downloading','monitoring','importing') THEN 1 ELSE 0 END) auto_active,
        SUM(CASE WHEN source='album' AND status='monitoring' THEN 1 ELSE 0 END) album_waiting,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status IN ('queued','searching','downloading','importing') THEN 1 ELSE 0 END) upgrades_active,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status='completed' THEN 1 ELSE 0 END) upgrades_done
        FROM requests""")
    lossy = db.one("""SELECT COUNT(*) n FROM (SELECT album_id FROM tracks GROUP BY album_id
        HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0)""")["n"]
    users = db.query("""SELECT u.id, u.username, u.auto_download, u.role,
        (SELECT COUNT(*) FROM playlists p WHERE p.user_id=u.id AND p.follow=1) followed,
        (SELECT COUNT(*) FROM connections c WHERE c.user_id=u.id) connections,
        (SELECT COUNT(*) FROM follows f WHERE f.user_id=u.id) follows
        FROM users u WHERE u.disabled=0 ORDER BY u.username""")
    return {"settings": {k: config.get(k) for k in AUTO_KEYS}, "users": users, "stats": {**stats, "lossy_albums": lossy}}


@router.put("/autodownload")
def put_autodownload(body: AutoDownload):
    config.set_many({k: v for k, v in body.settings.items() if k in AUTO_KEYS})
    for uid, on in body.users.items():
        db.execute("UPDATE users SET auto_download=? WHERE id=?", (int(on), int(uid)))
    return get_autodownload()


@router.post("/autodownload/upgrade-now")
async def upgrade_now():
    from .. import worker
    return {"queued": await worker.queue_lossy_upgrades(limit=50)}


@router.post("/autodownload/run-now")
async def run_now():
    import asyncio
    from ..services import sync
    asyncio.create_task(sync.sync_all())
    return {"ok": True}



# ---- scheduled tasks ---------------------------------------------------------------------------

class TaskPatch(BaseModel):
    enabled: bool | None = None
    minutes: int | None = None


@router.get("/tasks")
def list_tasks(admin=Depends(auth.admin_user)):
    from ..services import scheduler
    return {"tasks": scheduler.listing(), "cleanup_days": config.get("task_cleanup_days")}


@router.patch("/tasks/{key}")
def patch_task(key: str, body: TaskPatch, admin=Depends(auth.admin_user)):
    from ..services import scheduler
    if key not in scheduler.BY_KEY:
        raise HTTPException(404, "No such task")
    return scheduler.update(key, body.enabled, body.minutes)


@router.post("/tasks/{key}/run")
async def run_task(key: str, admin=Depends(auth.admin_user)):
    from ..services import scheduler
    if key not in scheduler.BY_KEY:
        raise HTTPException(404, "No such task")
    if not scheduler.run_now(key):
        raise HTTPException(409, "Already running")
    return {"ok": True}
