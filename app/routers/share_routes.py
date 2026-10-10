"""Public song links: anyone with the link can play that one song – nothing else – without an account."""
import os
import secrets
import time

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import FileResponse

from .. import auth, config, db
from . import library_routes

router = APIRouter(tags=["share"])
STATIC = os.path.join(os.path.dirname(os.path.dirname(__file__)), "static")


def _track(token: str) -> dict:
    s = db.one("SELECT track_id FROM shares WHERE token=?", (token,))
    t = db.one("SELECT id, title, artist, album, album_id, duration, format FROM tracks WHERE id=?", (s["track_id"],)) if s else None
    if not t:
        raise HTTPException(404, "This link doesn't work any more")
    return t


@router.post("/api/library/tracks/{track_id}/share")
def share(track_id: int, user=Depends(auth.current_user)):
    if not db.one("SELECT 1 FROM tracks WHERE id=?", (track_id,)):
        raise HTTPException(404, "Track not found")
    row = db.one("SELECT token FROM shares WHERE track_id=? AND user_id=?", (track_id, user["id"]))
    token = row["token"] if row else secrets.token_urlsafe(12)
    if not row:
        db.execute("INSERT INTO shares VALUES (?,?,?,?)", (token, track_id, user["id"], time.time()))
    return {"token": token, "path": f"/s/{token}", "url": (config.get("base_url") or "").rstrip("/") + f"/s/{token}"}


@router.get("/api/share/{token}")
def shared(token: str):
    t = _track(token)
    return {"title": t["title"], "artist": t["artist"], "album": t["album"], "duration": t["duration"], "app_name": config.get("app_name")}


@router.get("/api/share/{token}/stream")
async def shared_stream(token: str, transcode: bool = False, t: float = 0):
    return await library_routes.stream(_track(token)["id"], transcode, t, user=None)


@router.get("/api/share/{token}/cover")
async def shared_cover(token: str):
    return await library_routes.cover(_track(token)["album_id"], user=None)


@router.get("/s/{token}")
def share_page(token: str):
    return FileResponse(os.path.join(STATIC, "share.html"), headers={"Cache-Control": "no-cache"})
