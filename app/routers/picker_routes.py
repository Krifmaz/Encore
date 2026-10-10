"""Manual download picker: search one source, then download exactly the chosen result."""
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from .. import auth, config, db, worker
from ..services import picker
from .requests_routes import NewRequest, _create, _get_owned, _quota

router = APIRouter(prefix="/api/picker", tags=["picker"])


def _allowed(user: dict) -> None:
    if not auth.can(user, "pick"):
        raise HTTPException(403, "Ask an admin for permission to pick downloads")


class SearchBody(BaseModel):
    source: str
    kind: str = "album"          # album | track
    artist: str = ""
    title: str = ""
    query: str | None = None
    mbid: str | None = None
    artist_mbid: str | None = None
    track_count: int | None = None
    length: float | None = None


class PickBody(BaseModel):
    source: str                  # slskd | deezer | ytmusic | lidarr | link
    payload: dict
    title: str | None = None     # label of the chosen result
    request_id: int | None = None
    request: NewRequest | None = None


@router.get("/sources")
def sources(user=Depends(auth.current_user)):
    return {"sources": picker.available(), "allowed": auth.can(user, "pick")}


@router.post("/search")
async def search(body: SearchBody, user=Depends(auth.current_user)):
    _allowed(user)
    if body.kind not in ("album", "track"):
        raise HTTPException(400, "Manual search works for albums and songs")
    try:
        return await picker.search(body.source, body.kind, body.artist, body.title, body.query, body.mbid,
                                   body.track_count, body.length, body.artist_mbid)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"{body.source} search failed: {str(e)[:160]}")


@router.post("/pick")
async def pick(body: PickBody, user=Depends(auth.current_user)):
    """Create a request locked to the chosen result. An existing request is cancelled and replaced."""
    _allowed(user)
    if body.source == "link":
        url = (body.payload.get("url") or "").strip()
        if not url.startswith(("http://", "https://")):
            raise HTTPException(400, "Paste a full http(s) link")
        body.payload = {"url": url}
    try:
        payload = await picker.resolve(body.source, body.payload)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))
    if body.request_id:
        old = _get_owned(body.request_id, user)
        req = NewRequest(kind=old["kind"], mbid=old["mbid"], artist=old["artist"], title=old["title"], album=old["album"],
                         year=old["year"], cover_url=old["cover_url"], length=old["detail"].get("length"), isrc=old["detail"].get("isrc"))
        if old["status"] in ("pending", *worker.ACTIVE):
            await worker.cancel(old["id"])
            db.update_request(old["id"], status="cancelled", message="Replaced by a manual pick")
    elif body.request:
        req = body.request
        _quota(user)
    else:
        raise HTTPException(400, "Nothing to download")
    if req.kind not in ("album", "track"):
        raise HTTPException(400, "Manual picks work for albums and songs")
    label = body.title or payload.get("title") or ""
    rid = _create(user, req, extra={"pick": {"source": body.source, "payload": payload, "title": label}})
    r = db.get_request(rid)
    if r["status"] == "queued":
        db.update_request(rid, message=f"Manual pick: {label}"[:200] if label else "Manual pick")
    db.log("info", f"{user['username']} manually picked {body.source} result for {req.title or req.album}: {label}")
    return {"id": rid, "status": r["status"]}
