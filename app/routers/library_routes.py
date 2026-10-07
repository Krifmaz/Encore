import asyncio
import os
import tempfile
import time
import zipfile

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import FileResponse
from starlette.background import BackgroundTask
from pydantic import BaseModel

from .. import auth, config, db
from ..services import acoustid, library, tagger, verify

router = APIRouter(prefix="/api/library", tags=["library"])

SORTS = {
    "name": "lower(albumartist), year, lower(album)",
    "album": "lower(album)",
    "year": "year DESC, lower(albumartist)",
    "added": "added DESC",
    "artist": "lower(albumartist), year",
}

_bulk = {"running": False, "done": 0, "total": 0, "tagged": 0, "skipped": 0, "current": ""}


class Retag(BaseModel):
    release_id: str
    rename: bool | None = None


class TagEdit(BaseModel):
    track_ids: list[int]
    values: dict


@router.get("/stats")
def stats(user=Depends(auth.current_user)):
    return {**library.stats(), "scan": library.state, "bulk": _bulk, "verify": verify.state,
            "acoustid": acoustid.available()}


@router.post("/scan")
async def scan(full: bool = False, user=Depends(auth.admin_user)):
    if library.state["scanning"]:
        raise HTTPException(409, "A scan is already running")
    asyncio.create_task(library.scan(full=full))
    return {"ok": True}


@router.get("/albums")
def albums(q: str = "", sort: str = "name", filter: str = "", artist: str = "", limit: int = 120, offset: int = 0, user=Depends(auth.current_user)):
    where, params = ["1=1"], []
    if q:
        where.append("(album LIKE ? OR albumartist LIKE ? OR artist LIKE ?)")
        params += [f"%{q}%"] * 3
    if artist:
        where.append("lower(albumartist)=lower(?)")
        params.append(artist)
    having = ""
    if filter == "untagged":
        having = "HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0"
    elif filter == "tagged":
        having = "HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) = 0"
    elif filter == "lossless":
        having = "HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)"
    sql = f"""SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
              COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
              GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
              SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged
              FROM tracks WHERE {' AND '.join(where)} GROUP BY album_id {having}
              ORDER BY {SORTS.get(sort, SORTS['name'])} LIMIT ? OFFSET ?"""
    rows = db.query(sql, (*params, min(limit, 500), offset))
    total = db.one(f"SELECT COUNT(*) n FROM (SELECT album_id FROM tracks WHERE {' AND '.join(where)} GROUP BY album_id {having})", params)["n"]
    return {"total": total, "items": rows}


@router.get("/artists")
def artists(q: str = "", user=Depends(auth.current_user)):
    params = []
    where = ""
    if q:
        where = "WHERE albumartist LIKE ?"
        params.append(f"%{q}%")
    return db.query(f"""SELECT albumartist AS name, COUNT(DISTINCT album_id) albums, COUNT(*) tracks,
        MAX(mb_albumartistid) mbid, MIN(album_id) cover_album, SUM(size) size
        FROM tracks {where} GROUP BY lower(albumartist) ORDER BY lower(albumartist) LIMIT 2000""", params)


@router.get("/tracks")
def tracks(q: str = "", limit: int = 200, offset: int = 0, user=Depends(auth.current_user)):
    params: list = []
    where = ""
    if q:
        where = "WHERE title LIKE ? OR artist LIKE ? OR album LIKE ?"
        params += [f"%{q}%"] * 3
    return db.query(f"SELECT id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, verify_status, verify_detail FROM tracks {where} ORDER BY lower(artist), lower(album), disc, track LIMIT ? OFFSET ?",
                    (*params, min(limit, 1000), offset))


@router.get("/recent")
def recent(limit: int = 18, user=Depends(auth.current_user)):
    return db.query("""SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(added_at) added, COUNT(*) tracks
        FROM tracks GROUP BY album_id ORDER BY added DESC LIMIT ?""", (limit,))


@router.get("/albums/{album_id}")
def album(album_id: str, user=Depends(auth.current_user)):
    rows = db.query("SELECT * FROM tracks WHERE album_id=? ORDER BY disc, track, path", (album_id,))
    if not rows:
        raise HTTPException(404, "Album not found")
    lib = config.get("library_path").rstrip("/") + "/"
    for r in rows:
        r["relpath"] = r["path"][len(lib):] if r["path"].startswith(lib) else r["path"]
        if user["role"] != "admin":
            r.pop("path")
    first = rows[0]
    return {
        "id": album_id, "album": first["album"], "albumartist": first["albumartist"], "year": first["year"], "genre": first["genre"],
        "mb_albumid": first["mb_albumid"], "mb_releasegroupid": first["mb_releasegroupid"], "mb_albumartistid": first["mb_albumartistid"],
        "tracks": rows, "duration": sum(r["duration"] or 0 for r in rows), "size": sum(r["size"] or 0 for r in rows),
    }


_cover_misses: set[str] = set()


@router.get("/albums/{album_id}/cover")
async def cover(album_id: str, user=Depends(auth.current_user)):
    path = await asyncio.to_thread(library.find_cover, album_id)
    if not path and album_id not in _cover_misses:
        # fall back to the Cover Art Archive for MusicBrainz-tagged albums
        t = db.one("SELECT mb_albumid, mb_releasegroupid FROM tracks WHERE album_id=? AND (mb_albumid IS NOT NULL OR mb_releasegroupid IS NOT NULL) LIMIT 1", (album_id,))
        data = await tagger.fetch_cover(t["mb_albumid"], t["mb_releasegroupid"]) if t else None
        if data:
            path = os.path.join(db.DATA_DIR, "covers", album_id + ".jpg")
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as fh:
                fh.write(data)
        else:
            _cover_misses.add(album_id)
    if not path:
        raise HTTPException(404, "No cover")
    return FileResponse(path, headers={"Cache-Control": "private, max-age=86400"})


@router.get("/albums/{album_id}/download")
async def download_album(album_id: str, user=Depends(auth.current_user)):
    rows = db.query("SELECT path, album, albumartist FROM tracks WHERE album_id=? ORDER BY disc, track", (album_id,))
    if not rows:
        raise HTTPException(404, "Album not found")

    def build() -> str:
        fd, tmp = tempfile.mkstemp(suffix=".zip", prefix="encore-")
        os.close(fd)
        with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
            for r in rows:
                if os.path.isfile(r["path"]):
                    z.write(r["path"], os.path.basename(r["path"]))
            cov = library.find_cover(album_id)
            if cov and os.path.isfile(cov):
                z.write(cov, "cover" + os.path.splitext(cov)[1])
        return tmp

    tmp = await asyncio.to_thread(build)
    name = tagger.clean(f"{rows[0]['albumartist']} - {rows[0]['album']}") + ".zip"
    return FileResponse(tmp, media_type="application/zip", filename=name, background=BackgroundTask(os.remove, tmp))


@router.get("/albums/{album_id}/match")
async def match(album_id: str, artist: str | None = None, album: str | None = None, user=Depends(auth.admin_user)):
    try:
        return await tagger.candidates_for_album(album_id, artist, album)
    except ValueError as e:
        raise HTTPException(404, str(e))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")


@router.get("/albums/{album_id}/preview")
async def preview(album_id: str, release_id: str, user=Depends(auth.admin_user)):
    paths = [t["path"] for t in db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track, path", (album_id,))]
    if not paths:
        raise HTTPException(404, "Album not found")
    try:
        pv = await tagger.preview(paths, release_id)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    lib = config.get("library_path").rstrip("/") + "/"
    for row in pv["rows"]:
        row.pop("values", None)
        for k in ("path", "new_path"):
            if row.get(k, "").startswith(lib):
                row[k] = row[k][len(lib):]
    return pv


@router.post("/albums/{album_id}/retag")
async def retag(album_id: str, body: Retag, user=Depends(auth.admin_user)):
    try:
        paths = await tagger.retag_album(album_id, body.release_id, body.rename)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"Tagging failed: {e}")
    t = db.one("SELECT album_id FROM tracks WHERE path=?", (paths[0],)) if paths else None
    return {"ok": True, "album_id": t["album_id"] if t else None}


@router.post("/tags")
async def edit_tags(body: TagEdit, user=Depends(auth.admin_user)):
    try:
        paths = await asyncio.to_thread(tagger.edit_tags, body.track_ids, body.values)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"Could not write tags: {e}")
    await library.index_paths(paths)
    t = db.one("SELECT album_id FROM tracks WHERE path=?", (paths[0],)) if paths else None
    return {"ok": True, "album_id": t["album_id"] if t else None}


@router.delete("/albums/{album_id}")
async def delete_album(album_id: str, user=Depends(auth.admin_user)):
    rows = db.query("SELECT path FROM tracks WHERE album_id=?", (album_id,))
    dirs = set()
    for r in rows:
        try:
            os.remove(r["path"])
        except OSError:
            pass
        dirs.add(os.path.dirname(r["path"]))
        db.execute("DELETE FROM tracks WHERE path=?", (r["path"],))
    for d in dirs:
        tagger.prune_empty(d, config.get("library_path"))
    db.log("info", f"{user['username']} deleted album {album_id} ({len(rows)} files)")
    return {"ok": True}


@router.post("/autotag")
async def autotag_all(user=Depends(auth.admin_user)):
    if _bulk["running"]:
        raise HTTPException(409, "Auto-tagging already running")
    asyncio.create_task(_run_bulk())
    return {"ok": True}


async def _run_bulk():
    albums = db.query("""SELECT album_id, MAX(albumartist) artist, MAX(album) album FROM tracks
        GROUP BY album_id HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0""")
    _bulk.update(running=True, done=0, total=len(albums), tagged=0, skipped=0, current="")
    try:
        for a in albums:
            _bulk["current"] = f"{a['artist']} – {a['album']}"
            paths = [t["path"] for t in db.query("SELECT path FROM tracks WHERE album_id=?", (a["album_id"],))]
            try:
                pv = await tagger.auto_match(paths, a["artist"] if a["artist"] != "Unknown Artist" else "", a["album"] or "")
                if pv:
                    new = await tagger.apply_preview(pv)
                    for p in paths:
                        db.execute("DELETE FROM tracks WHERE path=?", (p,))
                    await library.index_paths(new)
                    _bulk["tagged"] += 1
                else:
                    _bulk["skipped"] += 1
            except Exception as e:  # noqa: BLE001
                _bulk["skipped"] += 1
                db.log("warning", f"Auto-tag failed for {_bulk['current']}: {e}")
            _bulk["done"] += 1
        db.log("info", f"Bulk auto-tag finished: {_bulk['tagged']} tagged, {_bulk['skipped']} need manual matching")
    finally:
        _bulk.update(running=False, current="")


# ---- verification & duplicates ----------------------------------------------

class DupResolve(BaseModel):
    key: str
    keep: int
    delete: list[int]


@router.post("/verify")
async def start_verify(all: bool = False, user=Depends(auth.admin_user)):
    if not acoustid.available():
        raise HTTPException(400, "Add an AcoustID API key in Settings → Verification (and make sure fpcalc is installed)")
    if verify.state["running"]:
        raise HTTPException(409, "Verification already running")
    asyncio.create_task(verify.verify_library(only_new=not all))
    return {"ok": True}


@router.post("/tracks/{track_id}/verify")
async def verify_one(track_id: int, user=Depends(auth.admin_user)):
    t = db.one("SELECT id, path, title, artist, duration, mb_trackid FROM tracks WHERE id=?", (track_id,))
    if not t:
        raise HTTPException(404, "Track not found")
    if not acoustid.available():
        raise HTTPException(400, "AcoustID is not configured")
    return await verify.verify_track(t)


@router.get("/mismatches")
def mismatches(user=Depends(auth.admin_user)):
    return db.query("""SELECT id, title, artist, album, album_id, verify_status, verify_detail FROM tracks
                       WHERE verify_status='mismatch' ORDER BY lower(artist), lower(album)""")


@router.get("/duplicates")
async def duplicates(user=Depends(auth.admin_user)):
    return await asyncio.to_thread(verify.find_duplicates)


@router.post("/duplicates/resolve")
async def resolve_duplicates(body: DupResolve, user=Depends(auth.admin_user)):
    if body.keep in body.delete:
        raise HTTPException(400, "Can't keep and delete the same track")
    n = await asyncio.to_thread(verify.delete_tracks, body.delete, body.keep)
    db.log("info", f"{user['username']} removed {n} duplicate file(s)")
    return {"deleted": n}


@router.post("/duplicates/resolve-all")
async def resolve_all(user=Depends(auth.admin_user)):
    groups = await asyncio.to_thread(verify.find_duplicates)
    n = 0
    for g in groups:
        n += await asyncio.to_thread(verify.delete_tracks, [t["id"] for t in g["tracks"] if t["id"] != g["keep"]], g["keep"])
    db.log("info", f"{user['username']} auto-resolved {len(groups)} duplicate groups ({n} files removed)")
    return {"groups": len(groups), "deleted": n}


@router.post("/duplicates/ignore")
def ignore_duplicates(body: dict, user=Depends(auth.admin_user)):
    db.execute("INSERT OR IGNORE INTO dup_ignore VALUES (?,?)", (body.get("key", ""), time.time()))
    return {"ok": True}
