import json
import time

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from .. import auth, config, db, worker
from ..services import notify

router = APIRouter(prefix="/api/requests", tags=["requests"])


EXT_SOURCES = ("deezer", "apple", "ytm", "bandcamp", "discogs")


class NewRequest(BaseModel):
    kind: str  # artist | album | track | url
    mbid: str | None = None
    artist: str | None = None
    title: str | None = None
    album: str | None = None
    year: str | None = None
    cover_url: str | None = None
    url: str | None = None
    length: float | None = None
    isrc: str | None = None
    album_mode: str | None = None   # None/"auto" = admin default · "song" = just this song · "album" = whole album
    ext: dict | None = None         # release MusicBrainz doesn't list: {"deezer": id, "apple": id, "ytm": browseId, "bandcamp": url, "discogs": "m123", "upc": ...}


def _quota(user: dict, n: int = 1) -> None:
    if user["role"] == "admin":
        return
    limit = user.get("request_limit") or 0
    if not limit:
        return
    used = db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND created_at > ? AND parent_id IS NULL", (user["id"], time.time() - 7 * 86400))["n"]
    if used + n > limit:
        raise HTTPException(429, f"Weekly request limit reached ({limit}). Ask an admin to raise it.")


def _create(user: dict, body: NewRequest, parent_id: int | None = None, notify_admin: bool = True, extra: dict | None = None) -> int:
    if body.kind not in ("artist", "album", "track", "url"):
        raise HTTPException(400, "Invalid request kind")
    if body.kind == "url":
        if not body.url or not body.url.startswith(("http://", "https://")):
            raise HTTPException(400, "A valid http(s) URL is required")
    elif body.kind == "album" and not body.mbid and body.ext and any(body.ext.get(k) for k in EXT_SOURCES):
        if not (body.artist and (body.title or body.album)):
            raise HTTPException(400, "Artist and album title are required")
        dup = db.one("""SELECT id, status FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                        AND status NOT IN ('failed','denied','cancelled')""", (body.artist, body.title or body.album))
        if dup and not extra:
            raise HTTPException(409, f"Already requested (#{dup['id']}, {dup['status']})")
    elif body.kind in ("artist", "album") and not body.mbid:
        raise HTTPException(400, "A MusicBrainz id is required")
    elif body.kind == "track" and not (body.mbid or (body.artist and body.title)):
        raise HTTPException(400, "Track needs a MusicBrainz id or artist + title")
    if body.kind == "track" and not body.mbid and not extra:
        dup = db.one("SELECT id, status FROM requests WHERE kind='track' AND lower(artist)=lower(?) AND lower(title)=lower(?) AND status NOT IN ('failed','denied','cancelled')",
                     (body.artist, body.title))
        if dup:
            raise HTTPException(409, f"Already requested (#{dup['id']}, {dup['status']})")
    if body.mbid and not extra:
        dup = db.one("SELECT id, status FROM requests WHERE mbid=? AND kind=? AND status NOT IN ('failed','denied','cancelled')", (body.mbid, body.kind))
        if dup:
            raise HTTPException(409, f"Already requested (#{dup['id']}, {dup['status']})")
    approved = user["role"] == "admin" or user.get("auto_approve") or config.get("auto_approve")
    now = time.time()
    detail = {"length": body.length} if body.length else {}
    if body.isrc:
        detail["isrc"] = body.isrc
    if body.album_mode in ("song", "album"):
        detail["album_mode"] = body.album_mode
    if body.ext:
        detail["ext"] = {k: body.ext[k] for k in (*EXT_SOURCES, "upc") if body.ext.get(k)}
    detail.update(extra or {})
    rid = db.execute(
        "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, url, status, message, detail, parent_id, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        (user["id"], body.kind, body.mbid, body.artist, body.title or body.album, body.album, body.year, body.cover_url, body.url,
         "queued" if approved else "pending", "" if approved else "Waiting for admin approval", json.dumps(detail), parent_id, now, now),
    )
    what = body.title or body.album or body.artist or body.url
    db.log("info", f"{user['username']} requested {body.kind}: {what}")
    return rid


@router.get("")
def list_requests(status: str | None = None, mine: bool = False, limit: int = 200, user=Depends(auth.current_user)):
    sql = "SELECT r.*, u.username FROM requests r LEFT JOIN users u ON u.id=r.user_id WHERE 1=1"
    params: list = []
    if user["role"] != "admin" or mine:
        sql += " AND r.user_id=?"
        params.append(user["id"])
    if status == "active":
        sql += " AND r.status IN ('pending','queued','searching','downloading','monitoring','importing')"
    elif status:
        sql += " AND r.status=?"
        params.append(status)
    sql += " ORDER BY r.id DESC LIMIT ?"
    params.append(min(limit, 1000))
    rows = db.query(sql, params)
    for r in rows:
        d = json.loads(r.pop("detail") or "{}")
        r["log"] = d.get("log", [])
        r["length"] = d.get("length")
        r["picked"] = bool(d.get("pick"))
        r["candidate"] = None
        s = d.get("slskd")
        if s and 0 <= s.get("idx", -1) < len(s.get("candidates", [])):
            c = s["candidates"][s["idx"]]
            r["candidate"] = {"username": c["username"], "format": c["format"], "directory": c["directory"].split("\\")[-1].split("/")[-1], "files": len(c["files"])}
    return rows


@router.post("")
async def create(body: NewRequest, user=Depends(auth.current_user)):
    _quota(user)
    rid = _create(user, body)
    r = db.get_request(rid)
    if r["status"] == "pending":
        await notify.send("🎵 New request awaiting approval", f"{user['username']} requested {body.kind}: {r['title'] or r['artist'] or r['url']}", body.cover_url)
    return r


class FromLink(BaseModel):
    url: str
    album_mode: str | None = None


@router.post("/from-link")
async def from_link(body: FromLink, user=Depends(auth.current_user)):
    """Request every song behind a link (one song, an album or a playlist) from the best source –
    optionally pulling each song's whole album."""
    from ..services import sources
    try:
        data = await sources.fetch(body.url, user["id"])
    except sources.SourceError as e:
        raise HTTPException(400, str(e))
    _quota(user)
    made, skipped = [], 0
    for t in data["tracks"][:500]:
        try:
            rid = _create(user, NewRequest(kind="track", artist=t["artist"], title=t["title"], album=t.get("album"),
                                           cover_url=t.get("cover"), length=t.get("duration"), isrc=t.get("isrc"),
                                           mbid=t.get("mbid"), album_mode=body.album_mode))
            made.append(rid)
        except HTTPException as e:
            if e.status_code != 409:
                raise
            skipped += 1
    window_status = db.get_request(made[0])["status"] if made else None
    return {"requested": len(made), "already_requested": skipped, "name": data.get("name"), "status": window_status,
            "ids": made}


class LinkBody(BaseModel):
    url: str
    album_mode: str | None = None


# links that only describe music (no downloadable audio) → matched song by song and fetched from the best source
_METADATA_ONLY = r"open\.spotify\.com|spotify\.link|^spotify:|music\.apple\.com|listenbrainz\.org|last\.fm/|tidal\.com|qobuz\.com|deezer\.com|deezer\.page\.link|music\.amazon\.|music\.youtube\.com/(playlist|browse)"


@router.post("/link")
async def link(body: LinkBody, user=Depends(auth.current_user)):
    """One box for any link: streaming-service links are matched song by song (best quality from any source);
    Qobuz/Tidal/Deezer rip directly when streamrip is set up; everything else (YouTube, SoundCloud, Bandcamp,
    Mixcloud and ~1,800 more sites) downloads as-is with yt-dlp."""
    import re

    from ..services import streamrip
    url = body.url.strip()
    if not url.startswith(("http://", "https://", "spotify:")):
        raise HTTPException(400, "Paste a full link (https://…)")
    rip = re.search(r"qobuz\.com|tidal\.com|deezer\.com", url) and streamrip.enabled()
    if re.search(_METADATA_ONLY, url, re.I) and not rip:
        r = await from_link(FromLink(url=url, album_mode=body.album_mode), user)
        return {**r, "mode": "matched"}
    _quota(user)
    rid = _create(user, NewRequest(kind="url", url=url))
    return {"requested": 1, "already_requested": 0, "ids": [rid], "status": db.get_request(rid)["status"], "mode": "direct"}


def _get_owned(rid: int, user: dict) -> dict:
    r = db.get_request(rid)
    if not r:
        raise HTTPException(404, "Request not found")
    if user["role"] != "admin" and r["user_id"] != user["id"]:
        raise HTTPException(403, "Not your request")
    return r


def _children(rid: int) -> list[int]:
    return [x["id"] for x in db.query("SELECT id FROM requests WHERE parent_id=?", (rid,))]


@router.post("/{rid}/approve")
def approve(rid: int, admin=Depends(auth.admin_user)):
    for i in [rid, *_children(rid)]:
        r = db.get_request(i)
        if r and r["status"] == "pending":
            db.update_request(i, status="completed" if r["kind"] == "playlist" else "queued", message="Approved")
    return {"ok": True}


@router.post("/{rid}/deny")
def deny(rid: int, admin=Depends(auth.admin_user)):
    for i in [rid, *_children(rid)]:
        r = db.get_request(i)
        if r and r["status"] == "pending":
            db.update_request(i, status="denied", message=f"Denied by {admin['username']}")
    return {"ok": True}


@router.post("/{rid}/retry")
def retry(rid: int, user=Depends(auth.current_user)):
    r = _get_owned(rid, user)
    if r["kind"] == "playlist":
        for c in db.query("SELECT id FROM requests WHERE parent_id=? AND status IN ('failed','cancelled')", (rid,)):
            db.update_request(c["id"], status="queued", progress=0, message="Retrying", source=None)
        return {"ok": True}
    if r["status"] not in ("failed", "cancelled", "completed"):
        raise HTTPException(400, "Only finished requests can be retried")
    if user["role"] != "admin" and not auth.can(user, "fix"):
        raise HTTPException(403, "Ask an admin to retry")
    if r["status"] == "cancelled" and user["role"] != "admin" and not (user.get("auto_approve") or config.get("auto_approve")):
        raise HTTPException(403, "Ask an admin to retry")
    db.update_request(rid, status="queued", progress=0, message="Retrying", source=None, detail={"length": r["detail"].get("length")} if r["detail"].get("length") else {})
    return {"ok": True}


@router.post("/{rid}/cancel")
async def cancel(rid: int, user=Depends(auth.current_user)):
    _get_owned(rid, user)  # permission check
    for i in [rid, *_children(rid)]:
        c = db.get_request(i)
        if c and c["status"] in ("pending", *worker.ACTIVE):
            await worker.cancel(i)
            db.update_request(i, status="cancelled", message=f"Cancelled by {user['username']}")
    return {"ok": True}


def _delete_files(r: dict) -> int:
    """Remove what a request left on disk: imported songs (completed) and staging/partial files (failed/cancelled)."""
    import os
    import shutil

    from ..services import verify
    n = 0
    ids = [t["id"] for t in db.query("SELECT id FROM tracks WHERE request_id IN (SELECT id FROM requests WHERE id=? OR parent_id=?)", (r["id"], r["id"]))]
    if ids:
        n += verify.delete_tracks(ids)
    for rid in [r["id"], *_children(r["id"])]:
        stage = os.path.join(config.get("staging_path"), f"request-{rid}")
        if os.path.isdir(stage):
            shutil.rmtree(stage, ignore_errors=True)
            n += 1
    st = (r["detail"] or {}).get("slskd") or {}
    if 0 <= st.get("idx", -1) < len(st.get("candidates", [])) and r["status"] != "completed":
        from ..services import slskd, tagger
        for f in st["candidates"][st["idx"]]["files"]:
            p = slskd.local_path(f["filename"])
            if p and os.path.exists(p):
                os.remove(p)
                tagger.prune_empty(os.path.dirname(p), config.get("slskd_download_path"))
                n += 1
    return n


@router.delete("/{rid}")
async def delete(rid: int, files: bool = False, user=Depends(auth.current_user)):
    """Remove a request from the list; files=true also deletes what it downloaded (admins only)."""
    r = _get_owned(rid, user)
    if files and user["role"] != "admin":
        raise HTTPException(403, "Only admins can delete files")
    if r["status"] in worker.ACTIVE:
        await worker.cancel(rid)
    n = _delete_files(r) if files else 0
    db.execute("DELETE FROM requests WHERE id=? OR parent_id=?", (rid, rid))
    if n:
        db.log("info", f"{user['username']} deleted request #{rid} and {n} file(s)")
    return {"ok": True, "deleted_files": n}


class ClearBody(BaseModel):
    statuses: list[str] = ["completed", "denied", "cancelled"]
    files: bool = False
    mine: bool = False


@router.post("/clear")
async def clear_finished(body: ClearBody | None = None, user=Depends(auth.current_user)):
    """Clear finished requests from the list (all users' for admins), optionally deleting their files."""
    body = body or ClearBody()
    statuses = [s for s in body.statuses if s in ("completed", "failed", "denied", "cancelled")]
    if not statuses:
        raise HTTPException(400, "Nothing to clear")
    if body.files and user["role"] != "admin":
        raise HTTPException(403, "Only admins can delete files")
    sql = f"SELECT id FROM requests WHERE parent_id IS NULL AND status IN ({','.join('?' * len(statuses))})"
    params: list = list(statuses)
    if user["role"] != "admin" or body.mine:
        sql += " AND user_id=?"
        params.append(user["id"])
    removed = files = 0
    for row in db.query(sql, params):
        r = db.get_request(row["id"])
        if body.files:
            files += _delete_files(r)
        db.execute("DELETE FROM requests WHERE id=? OR parent_id=?", (r["id"], r["id"]))
        removed += 1
    return {"ok": True, "removed": removed, "deleted_files": files}
