import asyncio
import os
import re
import tempfile
import time
import zipfile

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import FileResponse, StreamingResponse
from starlette.background import BackgroundTask
from pydantic import BaseModel

from .. import auth, config, db
from ..services import acoustid, library, tagger, verify

router = APIRouter(prefix="/api/library", tags=["library"])

# sort key → (SQL expression, default direction); "dir" flips it
SORTS = {
    "name": ("lower(albumartist) {d}, year, lower(album)", "asc"),
    "artist": ("lower(albumartist) {d}, year", "asc"),
    "album": ("lower(album) {d}", "asc"),
    "year": ("year {d}, lower(albumartist)", "desc"),
    "released": ("year {d}, lower(albumartist)", "desc"),
    "added": ("added {d}", "desc"),
    "duration": ("duration {d}", "desc"),
    "size": ("size {d}", "desc"),
}
TRACK_SORTS = {
    "artist": "lower(artist) {d}, lower(album), disc, track", "title": "lower(title) {d}", "album": "lower(album) {d}, disc, track",
    "added": "added_at {d}", "released": "year {d}, lower(artist)", "duration": "duration {d}",
}
ARTIST_SORTS = {"name": "lower(albumartist) {d}", "albums": "albums {d}", "tracks": "tracks {d}", "added": "added {d}"}


def _order(table: dict, key: str, direction: str, default: str) -> str:
    v = table.get(key) or table[default]
    expr, dflt = v if isinstance(v, tuple) else (v, "asc")
    d = direction if direction in ("asc", "desc") else dflt
    return expr.format(d=d.upper())

_bulk = {"running": False, "done": 0, "total": 0, "tagged": 0, "skipped": 0, "current": ""}


class Retag(BaseModel):
    release_id: str
    rename: bool | None = None


class TagEdit(BaseModel):
    track_ids: list[int]
    values: dict


@router.get("/stats")
def stats(user=Depends(auth.current_user)):
    return {**library.stats(), "scan": library.state, "bulk": _bulk, "verify": verify.state, "artwork": _art_job,
            "acoustid": acoustid.available()}


@router.post("/scan")
async def scan(full: bool = False, user=Depends(auth.admin_user)):
    if library.state["scanning"]:
        raise HTTPException(409, "A scan is already running")
    asyncio.create_task(library.scan(full=full))
    return {"ok": True}


_storage_cache: dict = {}


def _dir_size(path: str) -> int:
    total = 0
    for dp, _, fs in os.walk(path or "/nonexistent"):
        for f in fs:
            try:
                total += os.path.getsize(os.path.join(dp, f))
            except OSError:
                pass
    return total


@router.get("/storage")
async def storage(user=Depends(auth.current_user)):
    """Disk usage at a glance: disk, library by format/artist, biggest albums, download folders."""
    import shutil
    hit = _storage_cache.get("v")
    if hit and hit[0] > time.time():
        return hit[1]
    lib = config.get("library_path")
    try:
        du = shutil.disk_usage(lib)
        disk = {"total": du.total, "used": du.used, "free": du.free}
    except OSError:
        disk = None
    dl = {"soulseek": config.get("slskd_download_path"), "staging": config.get("staging_path")}
    sizes = await asyncio.gather(*[asyncio.to_thread(_dir_size, p) for p in dl.values()])
    out = {
        "disk": disk,
        "library": db.one("SELECT COALESCE(SUM(size),0) size, COUNT(*) tracks, COUNT(DISTINCT album_id) albums FROM tracks"),
        "formats": db.query("SELECT upper(format) format, COUNT(*) tracks, COALESCE(SUM(size),0) size FROM tracks GROUP BY format ORDER BY size DESC"),
        "artists": db.query("""SELECT MAX(albumartist) name, COALESCE(SUM(size),0) size, COUNT(*) tracks FROM tracks
                               GROUP BY lower(albumartist) ORDER BY size DESC LIMIT 15"""),
        "albums": db.query("""SELECT album_id id, MAX(album) album, MAX(albumartist) albumartist, COALESCE(SUM(size),0) size, COUNT(*) tracks
                              FROM tracks GROUP BY album_id ORDER BY size DESC LIMIT 12"""),
        "downloads": [{"name": k, "size": v} for k, v in zip(dl, sizes)],
    }
    _storage_cache["v"] = (time.time() + 60, out)
    return out


@router.get("/albums")
def albums(q: str = "", sort: str = "name", filter: str = "", artist: str = "", limit: int = 120, offset: int = 0,
           dir: str = "", kind: str = "", user=Depends(auth.current_user)):
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
    if kind in ("albums", "singles"):   # singles/loose songs = releases with 1–2 tracks
        cond = "COUNT(*) <= 2" if kind == "singles" else "COUNT(*) > 2"
        having = f"{having} AND {cond}" if having else f"HAVING {cond}"
    sql = f"""SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
              COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
              GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
              SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged
              FROM tracks WHERE {' AND '.join(where)} GROUP BY album_id {having}
              ORDER BY {_order(SORTS, sort, dir, 'name')} LIMIT ? OFFSET ?"""
    rows = db.query(sql, (*params, min(limit, 500), offset))
    total = db.one(f"SELECT COUNT(*) n FROM (SELECT album_id FROM tracks WHERE {' AND '.join(where)} GROUP BY album_id {having})", params)["n"]
    return {"total": total, "items": rows}


@router.get("/artists")
def artists(q: str = "", sort: str = "name", dir: str = "", user=Depends(auth.current_user)):
    params = []
    where = ""
    if q:
        where = "WHERE albumartist LIKE ?"
        params.append(f"%{q}%")
    return db.query(f"""SELECT albumartist AS name, COUNT(DISTINCT album_id) albums, COUNT(*) tracks,
        MAX(mb_albumartistid) mbid, MIN(album_id) cover_album, SUM(size) size, MAX(added_at) added
        FROM tracks {where} GROUP BY lower(albumartist) ORDER BY {_order(ARTIST_SORTS, sort, dir, 'name')} LIMIT 2000""", params)


@router.get("/tracks")
def tracks(q: str = "", limit: int = 200, offset: int = 0, sort: str = "artist", dir: str = "", user=Depends(auth.current_user)):
    params: list = []
    where = ""
    if q:
        where = "WHERE title LIKE ? OR artist LIKE ? OR album LIKE ?"
        params += [f"%{q}%"] * 3
    d = dir if dir in ("asc", "desc") else ("desc" if sort in ("added", "released", "duration") else "asc")
    order = TRACK_SORTS.get(sort, TRACK_SORTS["artist"]).format(d=d.upper())
    return db.query(f"""SELECT id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, verify_status, verify_detail, added_at
                        FROM tracks {where} ORDER BY {order} LIMIT ? OFFSET ?""", (*params, min(limit, 1000), offset))


@router.get("/tracks/{tid}/file")
def track_file(tid: int, user=Depends(auth.current_user)):
    """Download one song's file (no zip, no album)."""
    t = db.one("SELECT path, artist, title FROM tracks WHERE id=?", (tid,))
    if not t or not os.path.exists(t["path"]):
        raise HTTPException(404, "File not found")
    ext = os.path.splitext(t["path"])[1]
    name = re.sub(r'[\\/:*?"<>|]+', "_", f"{t['artist']} - {t['title']}{ext}")
    return FileResponse(t["path"], filename=name)


@router.get("/tracks/{tid}/lyrics")
async def track_lyrics(tid: int, fetch: bool = True, force: bool = False, user=Depends(auth.current_user)):
    from ..services import lyrics
    t = db.one("SELECT id, path, artist, title, album, duration FROM tracks WHERE id=?", (tid,))
    if not t:
        raise HTTPException(404, "Track not found")
    if not fetch:
        text, synced = lyrics.sidecar(t["path"])
        return {"lyrics": text, "synced": synced, "source": "file" if text else None}
    try:
        return await lyrics.fetch_for_track(t, force)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"Lyrics lookup failed: {e}")


@router.post("/albums/{album_id}/lyrics")
async def album_lyrics(album_id: str, user=Depends(auth.current_user)):
    from ..services import lyrics
    ids = [t["id"] for t in db.query("SELECT id FROM tracks WHERE album_id=?", (album_id,))]
    return await lyrics.fetch_missing(ids, limit=len(ids) or 1)


@router.get("/shuffle")
def shuffle(limit: int = 50, exclude: str = "", user=Depends(auth.current_user)):
    """Random songs from the whole library – what plays after your queue ends."""
    ex = [int(x) for x in exclude.split(",") if x.strip().isdigit()][:500]
    sql = "SELECT id, title, artist, album, album_id, format, bitrate, duration FROM tracks"
    if ex:
        sql += f" WHERE id NOT IN ({','.join('?' * len(ex))})"
    return db.query(sql + " ORDER BY RANDOM() LIMIT ?", (*ex, min(limit, 200)))


class PlayBody(BaseModel):
    track_id: int
    seconds: float = 0


@router.post("/plays")
def log_play(body: PlayBody, user=Depends(auth.current_user)):
    """Count a play (sent by the web player after 30 s or half the song)."""
    t = db.one("SELECT artist, title, album FROM tracks WHERE id=?", (body.track_id,))
    if not t:
        raise HTTPException(404, "Track not found")
    db.execute("INSERT INTO plays (user_id, track_id, artist, title, album, seconds, played_at) VALUES (?,?,?,?,?,?,?)",
               (user["id"], body.track_id, t["artist"], t["title"], t["album"], max(0, min(body.seconds, 7200)), time.time()))
    return {"ok": True}


@router.delete("/tracks/{tid}")
def delete_track(tid: int, user=Depends(auth.admin_user)):
    """Permanently delete one song's file (and its lyrics file)."""
    t = db.one("SELECT path, artist, title FROM tracks WHERE id=?", (tid,))
    if not t:
        raise HTTPException(404, "Track not found")
    for side in (".lrc", ".txt"):
        try:
            os.remove(os.path.splitext(t["path"])[0] + side)
        except OSError:
            pass
    verify.delete_tracks([tid])
    db.log("info", f"{user['username']} deleted {t['artist']} – {t['title']}")
    return {"ok": True}


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
        data = await _lookup_art(album_id)
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


async def _lookup_art(album_id: str) -> bytes | None:
    """Cover Art Archive for MusicBrainz-tagged albums, otherwise Deezer/iTunes by artist + album/title."""
    t = db.one("SELECT mb_albumid, mb_releasegroupid, albumartist, album, title, COUNT(*) OVER () n FROM tracks WHERE album_id=? LIMIT 1", (album_id,))
    if not t:
        return None
    data = await tagger.fetch_cover(t["mb_albumid"], t["mb_releasegroupid"]) if (t["mb_albumid"] or t["mb_releasegroupid"]) else None
    if not data:
        art = await tagger.find_art(t["albumartist"], t["album"], t["title"] if t["n"] == 1 else None)
        data = art[0] if art else None
    return data


async def _embed_album_art(album_id: str, data: bytes) -> int:
    rows = db.query("SELECT path FROM tracks WHERE album_id=?", (album_id,))

    def _go():
        n = 0
        for r in rows:
            try:
                tagger.embed_cover(r["path"], data)
                n += 1
            except Exception:  # noqa: BLE001
                pass
        return n
    n = await asyncio.to_thread(_go)
    await library.index_paths([r["path"] for r in rows])
    cache = os.path.join(db.DATA_DIR, "covers", album_id + ".jpg")
    os.makedirs(os.path.dirname(cache), exist_ok=True)
    with open(cache, "wb") as fh:
        fh.write(data)
    dirs = {os.path.dirname(r["path"]) for r in rows}
    if config.get("save_cover") and len(dirs) == 1:
        try:
            with open(os.path.join(dirs.pop(), "cover.jpg"), "wb") as fh:
                fh.write(data)
        except OSError:
            pass
    _cover_misses.discard(album_id)
    return n


@router.get("/albums/{album_id}/art-candidates")
async def art_candidates(album_id: str, q: str = "", user=Depends(auth.admin_user)):
    """Cover art choices from every catalogue (Cover Art Archive, Deezer, Apple Music, YouTube Music, Discogs)."""
    from ..services import art_search
    a = db.one("SELECT MAX(albumartist) artist, MAX(album) album, MAX(mb_releasegroupid) rg, MAX(mb_albumid) rel FROM tracks WHERE album_id=?", (album_id,))
    if not a or not a["artist"]:
        raise HTTPException(404, "Album not found")
    return await art_search.candidates(a["artist"], q or a["album"] or "", a["rg"], a["rel"])


class ArtBody(BaseModel):
    url: str | None = None
    data: str | None = None      # base64 image (upload)


@router.post("/albums/{album_id}/art")
async def set_album_art(album_id: str, body: ArtBody, user=Depends(auth.admin_user)):
    """Use a chosen or uploaded picture as the album's cover: embedded in every file + cover.jpg."""
    import base64

    from ..services.http import client
    if body.data:
        try:
            data = base64.b64decode(body.data.split(",", 1)[-1])
        except ValueError:
            raise HTTPException(400, "Invalid image")
    elif body.url and body.url.startswith(("http://", "https://")):
        r = await client().get(body.url, follow_redirects=True)
        if r.status_code >= 400:
            raise HTTPException(502, f"Could not download the picture ({r.status_code})")
        data = r.content
    else:
        raise HTTPException(400, "Choose a picture or upload one")
    if not (data[:3] == b"\xff\xd8\xff" or data[:8] == b"\x89PNG\r\n\x1a\n"):
        raise HTTPException(400, "Only JPEG and PNG pictures can be embedded")
    if len(data) > 15 * 1024 * 1024:
        raise HTTPException(400, "Picture is larger than 15 MB")
    n = await _embed_album_art(album_id, data)
    db.log("info", f"{user['username']} changed the artwork of album {album_id}")
    return {"embedded": n}


@router.post("/albums/{album_id}/artwork")
async def album_artwork(album_id: str, user=Depends(auth.admin_user)):
    """Find artwork online and embed it into the files (so Finamp/Plexamp show it too)."""
    data = await _lookup_art(album_id)
    if not data:
        raise HTTPException(404, "No artwork found on Cover Art Archive, Deezer or iTunes")
    return {"embedded": await _embed_album_art(album_id, data)}


_art_job = {"running": False, "done": 0, "total": 0, "found": 0}


async def find_missing_artwork() -> str:
    if _art_job["running"]:
        return "already running"
    _art_job.update(running=True, done=0, total=0, found=0)
    try:
        ids = [r["album_id"] for r in db.query("SELECT DISTINCT album_id FROM tracks")]
        missing = [a for a in ids if not await asyncio.to_thread(library.find_cover, a)]
        _art_job["total"] = len(missing)
        for a in missing:
            data = await _lookup_art(a)
            if data:
                await _embed_album_art(a, data)
                _art_job["found"] += 1
            _art_job["done"] += 1
        msg = f"found {_art_job['found']} of {_art_job['total']} missing covers"
        if missing:
            db.log("info", f"Artwork: {msg}")
        return msg
    finally:
        _art_job["running"] = False


@router.post("/artwork")
async def missing_artwork(user=Depends(auth.admin_user)):
    if _art_job["running"]:
        raise HTTPException(409, "Already running")
    asyncio.create_task(find_missing_artwork())
    return {"ok": True}


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


BROWSER_TYPES = {"mp3": "audio/mpeg", "flac": "audio/flac", "m4a": "audio/mp4", "mp4": "audio/mp4", "aac": "audio/aac",
                 "ogg": "audio/ogg", "opus": "audio/ogg", "oga": "audio/ogg", "wav": "audio/wav", "webm": "audio/webm"}


def _needs_transcode(path: str, fmt: str) -> bool:
    if fmt not in BROWSER_TYPES:
        return True                       # ape, wv, aiff, wma, alac files, …
    if fmt in ("m4a", "mp4"):
        try:
            import mutagen
            return getattr(mutagen.File(path).info, "codec", "") == "alac"   # Apple Lossless in .m4a
        except Exception:  # noqa: BLE001
            return False
    return False


@router.get("/tracks/{track_id}/stream")
async def stream(track_id: int, transcode: bool = False, t: float = 0, user=Depends(auth.current_user)):
    """Original file for formats browsers play natively (seekable, bit-perfect); others → FLAC on the fly."""
    track = db.one("SELECT path, format FROM tracks WHERE id=?", (track_id,))
    if not track or not os.path.isfile(track["path"]):
        raise HTTPException(404, "Track not found")
    if not transcode and not await asyncio.to_thread(_needs_transcode, track["path"], track["format"]):
        return FileResponse(track["path"], media_type=BROWSER_TYPES[track["format"]], headers={"Cache-Control": "private, max-age=3600"})
    proc = await asyncio.create_subprocess_exec(
        "ffmpeg", "-v", "error", *(["-ss", f"{max(t, 0):.2f}"] if t else []), "-i", track["path"],
        "-map", "0:a:0", "-c:a", "flac", "-f", "flac", "pipe:1",
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)

    async def body():
        try:
            while chunk := await proc.stdout.read(1 << 16):
                yield chunk
        finally:
            if proc.returncode is None:
                proc.kill()
    return StreamingResponse(body(), media_type="audio/flac", headers={"X-Transcoded": "flac"})


@router.get("/albums/{album_id}/match")
async def match(album_id: str, artist: str | None = None, album: str | None = None, provider: str = "musicbrainz", user=Depends(auth.admin_user)):
    from ..services import meta_providers
    if provider not in meta_providers.PROVIDERS:
        raise HTTPException(400, "Unknown metadata source")
    try:
        return await tagger.candidates_for_album(album_id, artist, album, provider)
    except ValueError as e:
        raise HTTPException(400 if "Album not found" not in str(e) else 404, str(e))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"{meta_providers.PROVIDERS[provider]} error: {e}")


@router.get("/albums/{album_id}/preview")
async def preview(album_id: str, release_id: str, user=Depends(auth.admin_user)):
    paths = [t["path"] for t in db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track, path", (album_id,))]
    if not paths:
        raise HTTPException(404, "Album not found")
    try:
        pv = await tagger.preview(paths, release_id)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"Metadata lookup failed: {e}")
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
        for p in (r["path"], os.path.splitext(r["path"])[0] + ".lrc", os.path.splitext(r["path"])[0] + ".txt"):
            try:
                os.remove(p)
            except OSError:
                pass
        dirs.add(os.path.dirname(r["path"]))
        db.execute("UPDATE playlist_items SET track_id=NULL WHERE track_id=(SELECT id FROM tracks WHERE path=?)", (r["path"],))
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
                    tagger.relink([r["path"] for r in pv["rows"]], new)
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
