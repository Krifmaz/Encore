import asyncio
import json

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from .. import auth, db
from ..services import follows

router = APIRouter(prefix="/api/follows", tags=["follows"])


class FollowBody(BaseModel):
    mbid: str
    name: str | None = None
    types: list[str] | None = None
    backfill: bool = False


class FollowPatch(BaseModel):
    types: list[str] | None = None


def _row(f: dict) -> dict:
    return {**{k: f[k] for k in ("id", "mbid", "name", "last_checked", "last_found", "found_count", "last_error", "created_at")},
            "types": json.loads(f["types"]), "known_releases": len(json.loads(f["seen"] or "[]"))}


@router.get("")
def list_follows(user=Depends(auth.current_user)):
    return [_row(f) for f in db.query("SELECT * FROM follows WHERE user_id=? ORDER BY lower(name)", (user["id"],))]


@router.post("")
async def follow(body: FollowBody, user=Depends(auth.current_user)):
    try:
        return await follows.follow(user, body.mbid, body.name, body.types, body.backfill)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")


@router.patch("/{fid}")
def update(fid: int, body: FollowPatch, user=Depends(auth.current_user)):
    f = db.one("SELECT * FROM follows WHERE id=? AND user_id=?", (fid, user["id"]))
    if not f:
        raise HTTPException(404, "Not following")
    if body.types is not None:
        types = [t for t in body.types if t in follows.TYPES] or ["Album"]
        db.execute("UPDATE follows SET types=? WHERE id=?", (json.dumps(types), fid))
    return _row(db.one("SELECT * FROM follows WHERE id=?", (fid,)))


@router.delete("/{fid}")
def unfollow(fid: int, user=Depends(auth.current_user)):
    db.execute("DELETE FROM follows WHERE id=? AND user_id=?", (fid, user["id"]))
    return {"ok": True}


@router.post("/check")
async def check_now(user=Depends(auth.current_user)):
    return {"new_releases": await follows.check_all(user["id"])}


_lib_job: dict = {}


@router.post("/library")
async def follow_library(user=Depends(auth.current_user)):
    """Follow every artist in the library (runs in the background – MusicBrainz allows 1 request/second)."""
    if _lib_job.get(user["id"], {}).get("running"):
        raise HTTPException(409, "Already running")
    _lib_job[user["id"]] = {"running": True}

    async def run():
        try:
            _lib_job[user["id"]] = {"running": False, **await follows.follow_library_artists(user)}
        except Exception as e:  # noqa: BLE001
            _lib_job[user["id"]] = {"running": False, "error": str(e)}
    asyncio.create_task(run())
    return {"ok": True}


@router.get("/library")
def follow_library_status(user=Depends(auth.current_user)):
    return _lib_job.get(user["id"], {"running": False})
