"""Request pipeline: Lidarr → slskd → yt-dlp, with import + tagging."""
import asyncio
import os
import re
import shutil
import time
import traceback

from . import config, db
from .services.http import loop_local
from .services import library, lidarr, musicbrainz as mb, notify, slskd, streamrip, tagger, ytdlp, ytmusic

ACTIVE = ("queued", "searching", "downloading", "monitoring", "importing")
_running: dict[int, asyncio.Task] = {}
_stop = asyncio.Event()


class Cancelled(Exception):
    pass


def _check(rid: int) -> dict:
    r = db.get_request(rid)
    if not r or r["status"] not in ACTIVE:
        raise Cancelled()
    return r


def upd(r: dict, **kw) -> None:
    if "detail" not in kw:
        kw["detail"] = r["detail"]
    db.update_request(r["id"], **kw)
    r.update(kw)


def label(r: dict) -> str:
    if r["kind"] == "album":
        return f"{r['artist']} – {r['album'] or r['title']}"
    if r["kind"] == "artist":
        return r["artist"]
    if r["kind"] == "url":
        return r["title"] or r["url"]
    return f"{r['artist']} – {r['title']}"


# ---- lifecycle --------------------------------------------------------------

def record_origin(paths: list[str], origin: dict | None, request_id: int | None, since: float = 0) -> None:
    """Remember where each file came from (shown on the album page). Files that were already in the library
    (kept instead of a new duplicate) keep their original origin."""
    if not origin:
        return
    now = time.time()
    for p in paths:
        db.execute("""UPDATE tracks SET source=?, source_detail=?, source_url=?, request_id=?, downloaded_at=?
                      WHERE path=? AND (downloaded_at IS NULL OR downloaded_at >= ?) AND added_at >= ?""",
                   (origin.get("source"), origin.get("detail"), origin.get("url"), request_id, now, p, since, since - 1))


async def complete(r: dict, message: str, new_paths: list[str] | None = None, origin: dict | None = None) -> None:
    if new_paths:
        await library.index_paths(new_paths)
        record_origin(new_paths, origin, r["id"], r.get("created_at") or 0)
    upd(r, status="completed", progress=100, message=message)
    try:
        from .services import playlists
        playlists.rematch()
    except Exception:  # noqa: BLE001
        traceback.print_exc()
    db.log("info", f"Request completed: {label(r)} ({message})")
    await notify.send("✅ Now available", f"{label(r)}\n{message}", r.get("cover_url"))
    if new_paths:
        await notify.refresh_media_servers()


async def fail(r: dict, message: str) -> None:
    if r["detail"].get("lossless_only"):
        upd(r, status="failed", message="No lossless copy available yet – will try again later")
        return
    upd(r, status="failed", message=message)
    db.log("error", f"Request failed: {label(r)} – {message}")
    await notify.send("❌ Request failed", f"{label(r)}\n{message}", r.get("cover_url"))


async def next_source(r: dict, reason: str = "") -> None:
    d = r["detail"]
    srcs = d.get("sources", [])
    if reason:
        d.setdefault("log", []).append(f"{r.get('source') or 'start'}: {reason}")
    if not srcs:
        tried = "; ".join(d.get("log", [])[-3:]) or "no download sources enabled"
        await fail(r, f"All sources exhausted ({tried})")
        return
    src = srcs.pop(0)
    d["sources"] = srcs
    upd(r, source=src, progress=0, status="searching", message=f"Trying {src}…")
    if src == "slskd_lossless":
        await start_slskd(r, lossless_only=True)
        return
    await {"lidarr": start_lidarr, "slskd": start_slskd, "ytdlp": start_ytdlp, "streamrip": start_streamrip}[src](r)


async def start(r: dict) -> None:
    d = r["detail"]
    if r["kind"] == "playlist":
        upd(r, status="completed")
        return
    if r["kind"] == "url":
        if re.search(r"qobuz\.com|tidal\.com|deezer\.com", r["url"] or ""):
            if not streamrip.enabled():
                await fail(r, "Qobuz/Tidal/Deezer links need streamrip enabled in Settings → Downloaders")
                return
            upd(r, source="streamrip", status="downloading", message="Downloading with streamrip…")
            await run_streamrip_url(r)
            return
        upd(r, source="ytdlp", status="downloading", message="Starting download…")
        await run_url(r)
        return
    if d.get("pick"):
        await start_pick(r)
        return
    # already have it? (lossless upgrade requests replace what's there)
    if r["kind"] == "album" and not d.get("lossless_only") and (library.has_release_group(r["mbid"]) or library.has_album(r["artist"], r["album"] or r["title"])):
        await complete(r, "Already in library")
        return
    if r["kind"] == "track" and _track_in_library(r):
        await complete(r, "Already in library")
        return
    if r["kind"] == "track" and await try_album_upgrade(r):
        return
    if r["kind"] == "artist":
        if lidarr.enabled():
            upd(r, source="lidarr", status="searching", message="Adding artist to Lidarr…")
            a = await lidarr.ensure_artist(r["mbid"], monitor_all=True)
            await complete(r, f"Added to Lidarr – monitoring all albums ({a.get('artistName', r['artist'])})")
        else:
            await expand_artist(r)
        return
    if r["kind"] == "album" and not r["mbid"] and d.get("ext"):
        await _release_info(r)      # Deezer/Apple tracklist: used for matching downloads and tagging
    order = [s.strip() for s in config.get("source_order").split(",") if s.strip()]
    if config.get("quality_first") or d.get("lossless_only"):
        # every lossless option first (in the admin's order), then lossy fallbacks
        lossless = [s for s in order if s == "streamrip"] + (["slskd_lossless"] if "slskd" in order else [])
        order = lossless + [s for s in order if s not in ("streamrip",)]
    if d.get("lossless_only"):
        order = [s for s in order if s in ("streamrip", "slskd_lossless")]
    avail = {"lidarr": lidarr.enabled() and r["kind"] == "album" and r["mbid"], "slskd": slskd.enabled(),
             "slskd_lossless": slskd.enabled(), "ytdlp": ytdlp.enabled(), "streamrip": streamrip.enabled()}
    seen: set = set()
    d["sources"] = [s for s in order if avail.get(s) and not (s in seen or seen.add(s))]
    d["log"] = []
    await next_source(r)


async def expand_artist(r: dict) -> None:
    upd(r, status="searching", message="Looking up discography…")
    art = await mb.artist(r["mbid"])
    albums = [g for g in art["release_groups"] if g["type"] in ("Album", "EP") and not g["secondary"]]
    now = time.time()
    n = 0
    for g in albums:
        if library.has_release_group(g["id"]) or db.one("SELECT 1 FROM requests WHERE mbid=? AND status NOT IN ('failed','denied','cancelled')", (g["id"],)):
            continue
        db.execute(
            "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, detail, parent_id, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (r["user_id"], "album", g["id"], art["name"], g["title"], g["title"], g["year"], g["cover"], "queued", "{}", r["id"], now, now),
        )
        n += 1
    await complete(r, f"Queued {n} album request(s) from the discography")


# ---- song → whole album ------------------------------------------------------

def _track_in_library(r: dict) -> bool:
    from .services.matcher import LibraryIndex, describe
    d = r["detail"]
    hit, _ = LibraryIndex.load().find(describe(r["title"], r["artist"], length=d.get("length"), isrc=d.get("isrc"),
                                               mbid=r["mbid"] or d.get("recording")))
    return bool(hit)


async def try_album_upgrade(r: dict) -> bool:
    """If the song belongs to an official album, fetch the whole album instead (best quality available)."""
    d = r["detail"]
    mode = {"song": "off", "album": "album_ep"}.get(d.get("album_mode") or "", config.get("album_upgrade"))
    if mode == "off" or d.get("no_album"):
        return False
    upd(r, status="searching", message="Looking up which album this song is on…")
    try:
        rid = r["mbid"] or await mb.resolve_recording(r["artist"], r["title"], d.get("length"), d.get("isrc"))
        alb = await mb.album_for_recording(rid, allow_ep=mode == "album_ep") if rid else None
    except Exception as e:  # noqa: BLE001
        db.log("warning", f"Album lookup failed for {label(r)}: {e}")
        alb, rid = None, None
    d["recording"] = rid
    if not alb or library.has_release_group(alb["id"]):
        d["no_album"] = True
        upd(r)
        return False
    existing = db.one("SELECT id FROM requests WHERE kind='album' AND mbid=? AND status NOT IN ('failed','denied','cancelled')", (alb["id"],))
    if existing:
        aid = existing["id"]
    else:
        now = time.time()
        aid = db.execute(
            "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (r["user_id"], "album", alb["id"], alb["artist"] or r["artist"], alb["title"], alb["title"], alb["year"], alb["cover"],
             "queued", f"Full album for “{r['title']}”", "{}", now, now))
        db.log("info", f"“{r['title']}” is on {alb['type'].lower()} “{alb['title']}” – downloading the whole {alb['type'].lower()}")
    d["album_request"] = aid
    upd(r, status="monitoring", source="album", cover_url=r.get("cover_url") or alb["cover"],
        message=f"Getting the whole {alb['type'].lower()} “{alb['title']}”")
    return True


async def poll_album(r: dict) -> None:
    d = r["detail"]
    a = db.get_request(d.get("album_request") or 0)
    if a and a["status"] == "completed":
        if _track_in_library(r):
            await complete(r, f"Downloaded with the album “{a['title']}”")
            return
        reason = "the album didn't contain this exact version"
    elif not a or a["status"] in ("failed", "denied", "cancelled"):
        reason = "the album couldn't be downloaded"
    else:
        if a["progress"] != r["progress"] or a["message"] != d.get("last_msg"):
            d["last_msg"] = a["message"]
            upd(r, progress=a["progress"], message=f"Album “{a['title']}”: {a['message'] or a['status']}")
        return
    d["no_album"] = True
    upd(r, status="queued", source=None, message=f"Getting the song on its own ({reason})")


# ---- lossless upgrades ---------------------------------------------------------

async def retag_external() -> int:
    """Albums imported from Deezer/Apple metadata (not in MusicBrainz at the time): once someone adds the
    release to MusicBrainz (e.g. through Harmony), re-tag it properly. Runs daily."""
    done = 0
    rows = db.query("""SELECT t.album_id, MIN(t.albumartist) artist, MIN(t.album) album FROM tracks t JOIN requests r ON r.id=t.request_id
                       WHERE (t.mb_albumid IS NULL OR t.mb_albumid='') AND r.mbid IS NULL AND r.detail LIKE '%"ext"%'
                       GROUP BY t.album_id LIMIT 25""")
    for a in rows:
        paths = [t["path"] for t in db.query("SELECT path FROM tracks WHERE album_id=?", (a["album_id"],))]
        try:
            pv = await tagger.auto_match(paths, a["artist"], a["album"])
            if pv:
                await tagger.apply_preview(pv, True)
                db.log("info", f"{a['artist']} – {a['album']} is in MusicBrainz now: re-tagged")
                done += 1
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Re-tag of {a['album']} failed: {e}")
    return done


async def queue_lossy_upgrades(limit: int = 20) -> int:
    """Re-request MusicBrainz-tagged albums that are only in lossy quality, lossless sources only."""
    days = int(config.get("upgrade_lossy_days"))
    rows = db.query("""SELECT mb_releasegroupid rg, MAX(albumartist) artist, MAX(album) album, MAX(year) year FROM tracks
                       WHERE mb_releasegroupid IS NOT NULL GROUP BY album_id
                       HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0""")
    n = 0
    now = time.time()
    for a in rows:
        if n >= limit:
            break
        last = db.one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", (f"upg:{a['rg']}",))
        if last and now - float(last["value"]) < days * 86400:
            continue
        if db.one("SELECT 1 FROM requests WHERE mbid=? AND status IN ('queued','searching','downloading','monitoring','importing','pending')", (a["rg"],)):
            continue
        db.execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", (f"upg:{a['rg']}", str(now)))
        db.execute(
            "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at) VALUES (NULL,'album',?,?,?,?,?,?,'queued',?,?,?,?)",
            (a["rg"], a["artist"], a["album"], a["album"], a["year"], mb.cover_rg(a["rg"]), "Lossless upgrade",
             '{"lossless_only": true}', now, now))
        n += 1
    if n:
        db.log("info", f"Queued {n} album(s) for a lossless upgrade")
    return n


# ---- Lidarr -----------------------------------------------------------------

async def start_lidarr(r: dict) -> None:
    a = await lidarr.ensure_artist(r["detail"].get("artist_mbid") or await _artist_mbid(r))
    r["detail"]["lidarr"] = {"artist_id": a["id"], "stage": "album", "started": time.time()}
    upd(r, status="monitoring", message="Waiting for Lidarr to load the album…")
    try:
        await lidarr.refresh_artist(a["id"])
    except lidarr.LidarrError:
        pass


async def _artist_mbid(r: dict) -> str:
    rg = await mb.release_group(r["mbid"])
    r["detail"]["artist_mbid"] = rg["artist_id"]
    return rg["artist_id"]


async def poll_lidarr(r: dict) -> None:
    st = r["detail"]["lidarr"]
    if st.get("next", 0) > time.time():
        return
    st["next"] = time.time() + 30
    if st["stage"] == "album":
        alb = await lidarr.album_by_rg(r["mbid"])
        if alb:
            await lidarr.monitor_and_search(alb["id"])
            st.update(stage="search", album_id=alb["id"], started=time.time())
            upd(r, message="Lidarr is searching indexers…")
        elif time.time() - st["started"] > 300:
            await next_source(r, "album not available in Lidarr (check metadata profile)")
        else:
            upd(r)
        return
    alb = await lidarr.album(st["album_id"])
    stats = alb.get("statistics") or {}
    if stats.get("trackFileCount", 0) > 0 and stats.get("percentOfTracks", 0) >= 99:
        await library.scan()
        now = time.time()
        db.execute("""UPDATE tracks SET source='Lidarr', source_detail=?, request_id=?, downloaded_at=?
                      WHERE mb_releasegroupid=? AND source IS NULL""", (f"Lidarr album #{st['album_id']}", r["id"], now, r["mbid"]))
        await complete(r, "Downloaded and imported by Lidarr")
        return
    q = [x for x in await lidarr.queue() if x["album"] == alb.get("title")]
    if q:
        upd(r, status="downloading", progress=q[0]["progress"], message=f"Lidarr: {q[0]['state'] or q[0]['status']} via {q[0]['client'] or q[0]['protocol']}")
        st["started"] = time.time()  # active download, don't time out
        return
    if time.time() - st["started"] > int(config.get("lidarr_timeout_minutes")) * 60:
        await next_source(r, "Lidarr found nothing in time (album stays monitored in Lidarr)")
    else:
        upd(r)


# ---- slskd ------------------------------------------------------------------

async def start_pick(r: dict) -> None:
    """Manual pick: download exactly the chosen result – no fallback to other sources."""
    d = r["detail"]
    p = d["pick"]
    d["sources"], d["log"] = [], []
    src = p["source"]
    if src == "slskd":
        d["slskd"] = {"candidates": [p["payload"]], "idx": -1}
        upd(r, source="slskd")
        await _slskd_try_next(r)
    elif src == "deezer" or (src == "link" and re.search(r"qobuz\.com|tidal\.com|deezer\.com", p["payload"]["url"])):
        if not streamrip.enabled():
            await fail(r, "Qobuz/Tidal/Deezer links need streamrip enabled in Settings → Downloaders")
            return
        upd(r, source="streamrip")
        await start_streamrip(r, urls=[p["payload"]["url"]])
    elif src in ("ytmusic", "link"):
        upd(r, source="ytdlp")
        await start_ytdlp(r, pick_url=p["payload"]["url"])
    elif src == "lidarr":
        pl = p["payload"]
        await lidarr.grab(pl["guid"], pl["indexerId"])
        d["lidarr"] = {"stage": "search", "album_id": pl["album_id"], "started": time.time()}
        upd(r, source="lidarr", status="monitoring", message=f"Lidarr is grabbing {p.get('title') or 'the chosen release'}…")
    else:
        await fail(r, f"Unknown source {src}")


async def _release_info(r: dict) -> dict | None:
    d = r["detail"]
    if "release" not in d and r["kind"] == "album" and not r["mbid"] and d.get("ext"):
        from .services import catalog   # not in MusicBrainz: tracklist from Deezer/Apple
        try:
            tracks = await catalog.tracklist(d["ext"])
            d["release"] = {"id": None, "tracks": tracks} if tracks else None
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Tracklist lookup failed for {label(r)}: {e}")
            d["release"] = None
    if "release" not in d and r["kind"] == "album" and r["mbid"]:
        try:
            rid = await mb.best_release_for_group(r["mbid"])
            rel = await mb.release(rid) if rid else None
            d["release"] = {"id": rel["id"], "tracks": [{"title": t["title"], "artist": t["artist"], "length": t["length"], "recording_id": t["recording_id"]} for t in rel["tracks"]]} if rel else None
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"MusicBrainz release lookup failed: {e}")
            d["release"] = None
    return d.get("release")


def _source_meta(r: dict) -> dict | None:
    """Tracklist/year from Deezer or Apple for releases MusicBrainz doesn't have (used to tag the files)."""
    d = r["detail"]
    if r["mbid"] or not d.get("ext"):
        return None
    return {"tracks": (d.get("release") or {}).get("tracks") or [], "year": r.get("year"), "upc": d["ext"].get("upc")}


def _q(*parts: str) -> str:
    import re
    s = " ".join(p for p in parts if p)
    s = re.sub(r"[\(\[].*?[\)\]]", " ", s)
    s = re.sub(r"[^\w\s']", " ", s)
    return " ".join(s.split())


async def start_slskd(r: dict, lossless_only: bool = False) -> None:
    upd(r, status="searching", message="Searching Soulseek for lossless…" if lossless_only else "Searching Soulseek…")
    if r["kind"] == "album":
        rel = await _release_info(r)
        n = len(rel["tracks"]) if rel else None
        album = r["album"] or r["title"]
        cands = []
        for query in (_q(r["artist"], album), _q(album)):
            responses = await slskd.search(query)
            cands = slskd.rank_album(responses, r["artist"], album, n, lossless_only)
            if cands:
                break
    else:
        length = r["detail"].get("length")
        responses = await slskd.search(_q(r["artist"], r["title"]))
        cands = slskd.rank_track(responses, r["artist"], r["title"], length, lossless_only)
    _check(r["id"])
    tried = set(r["detail"].get("slskd_tried", []))
    cands = [c for c in cands if f"{c['username']}|{c['directory']}" not in tried]
    if not cands:
        await next_source(r, "no lossless results on Soulseek" if lossless_only else "no suitable results on Soulseek")
        return
    r["detail"]["slskd"] = {"candidates": cands, "idx": -1}
    await _slskd_try_next(r)


async def _slskd_try_next(r: dict, reason: str = "") -> None:
    st = r["detail"]["slskd"]
    if st["idx"] >= 0:
        c = st["candidates"][st["idx"]]
        r["detail"].setdefault("slskd_tried", []).append(f"{c['username']}|{c['directory']}")
        for f in await slskd.user_downloads(c["username"]):
            if f.get("filename") in {x["filename"] for x in c["files"]} and not f.get("state", "").startswith("Completed"):
                await slskd.cancel(c["username"], f["id"])
    st["idx"] += 1
    if st["idx"] >= len(st["candidates"]):
        await next_source(r, reason or "all Soulseek candidates failed")
        return
    c = st["candidates"][st["idx"]]
    try:
        await slskd.enqueue(c["username"], c["files"])
    except slskd.SlskdError as e:
        await _slskd_try_next(r, str(e))
        return
    st.update(started=time.time(), last_change=time.time(), last_bytes=0)
    upd(r, status="downloading", progress=0,
        message=f"Downloading {len(c['files'])} file(s) from {c['username']} ({c['format'].upper()}{' ' + str(c['bitrate']) + 'k' if c['bitrate'] else ''})")


async def poll_slskd(r: dict) -> None:
    st = r["detail"]["slskd"]
    c = st["candidates"][st["idx"]]
    wanted = {f["filename"] for f in c["files"]}
    # one entry per file: re-queued files show up again in slskd's list, the newest one counts
    latest: dict = {}
    for f in await slskd.user_downloads(c["username"]):
        if f.get("filename") in wanted:
            latest[f["filename"]] = f
    transfers = list(latest.values())
    if not transfers:
        if time.time() - st["started"] > 120:
            await _slskd_try_next(r, "transfers disappeared")
        return
    states = [f.get("state", "") for f in transfers]
    if any(any(b in s for b in slskd.DONE_BAD) for s in states):
        db.log("warning", f"Soulseek transfer from {c['username']} failed for {label(r)}; trying next source")
        await _slskd_try_next(r, "transfer failed")
        return
    total = sum(f.get("size", 0) for f in transfers) or 1
    got = sum(f.get("bytesTransferred", 0) for f in transfers)
    if got > st.get("last_bytes", 0):
        st.update(last_bytes=got, last_change=time.time())
    done = sum(1 for s in states if slskd.DONE_OK in s)
    if done == len(wanted):
        upd(r, status="importing", progress=100, message="Importing…")
        await import_slskd(r, c)
        return
    if time.time() - st["last_change"] > int(config.get("slskd_stall_minutes")) * 60:
        await _slskd_try_next(r, "transfer stalled")
        return
    queued = all(s.startswith("Queued") for s in states)
    upd(r, progress=round(got / total * 100, 1),
        message=f"{'Queued by' if queued else 'Downloading from'} {c['username']} – {done}/{len(wanted)} files")


async def import_slskd(r: dict, c: dict) -> None:
    paths = []
    for attempt in range(6):
        paths = [slskd.local_path(f["filename"]) for f in c["files"]]
        if all(paths):
            break
        await asyncio.sleep(5)
    paths = [p for p in paths if p]
    if not paths:
        await fail(r, f"Downloaded files not found under {config.get('slskd_download_path')} – check the shared volume mapping")
        return
    root = config.get("slskd_download_path")
    note = ""
    try:
        if r["kind"] == "album":
            new = await tagger.import_album_files(paths, r["artist"], r["album"] or r["title"], r["mbid"], root, source_meta=_source_meta(r))
        else:
            p, note = await tagger.import_single(paths[0], r["artist"], r["title"], r["mbid"], root, r["detail"].get("length"))
            new = [p]
    except tagger.VerificationFailed as e:
        _discard(paths, root)
        db.log("warning", f"Rejected download from {c['username']} for {label(r)}: {e}")
        upd(r, status="downloading", message=f"Rejected {c['username']}'s files: {e}")
        await _slskd_try_next(r, f"wrong audio ({e})")
        return
    await complete(r, f"Downloaded from Soulseek ({c['format'].upper()})" + (f" – {note}" if note else ""), new,
                   {"source": "Soulseek", "detail": f"user {c['username']} · {c['directory'].replace(chr(92), '/')}"})


def _discard(paths: list[str], root: str) -> None:
    for p in paths:
        try:
            os.remove(p)
        except OSError:
            pass
        tagger.prune_empty(os.path.dirname(p), root)


# ---- streamrip (Qobuz / Tidal / Deezer) ---------------------------------------



async def _isrc_for(r: dict) -> str | None:
    d = r["detail"]
    if d.get("isrc"):
        return d["isrc"]
    if r["kind"] == "track" and r["mbid"]:
        try:
            d["isrc"] = (await mb.recording(r["mbid"])).get("isrc")
        except Exception:  # noqa: BLE001
            pass
    return d.get("isrc")


async def start_streamrip(r: dict, urls: list[str] | None = None) -> None:
    async with loop_local("streamrip", lambda: asyncio.Semaphore(1)):
        _check(r["id"])
        upd(r, status="searching", message="Looking for an exact lossless match…")
        if not urls and r["detail"].get("ext", {}).get("deezer") and "deezer" in config.get("streamrip_sources"):
            urls = [f"https://www.deezer.com/album/{r['detail']['ext']['deezer']}"]
        if urls:
            pass
        elif r["kind"] == "album":
            rel = await _release_info(r)
            first_isrc = None
            if rel and r["mbid"]:
                try:
                    full = await mb.release(rel["id"])
                    first_isrc = next((t["isrc"] for t in full["tracks"] if t.get("isrc")), None)
                except Exception:  # noqa: BLE001
                    pass
            urls = await streamrip.resolve_album(r["artist"], r["album"] or r["title"], len(rel["tracks"]) if rel else None, first_isrc)
        else:
            urls = await streamrip.resolve_track(r["artist"], r["title"], await _isrc_for(r), r["detail"].get("length"))
        if not urls:
            await next_source(r, "not found on Deezer/Tidal")
            return
        last_err = ""
        for url in urls:
            _check(r["id"])
            out = os.path.join(_staging(r), "streamrip")
            shutil.rmtree(out, ignore_errors=True)
            upd(r, status="downloading", message=f"streamrip: {url}")
            try:
                files = await streamrip.download(url, out)
                upd(r, status="importing", progress=100, message="Verifying, tagging and importing…")
                if r["kind"] == "album":
                    new = await tagger.import_album_files(files, r["artist"], r["album"] or r["title"], r["mbid"], out, source_meta=_source_meta(r))
                    note = ""
                else:
                    p, note = await tagger.import_single(files[0], r["artist"], r["title"], r["mbid"], out, r["detail"].get("length"))
                    new = [p]
                shutil.rmtree(_staging(r), ignore_errors=True)
                svc = "Deezer" if "deezer" in url else "Tidal" if "tidal" in url else "Qobuz"
                await complete(r, f"Downloaded from {svc} via streamrip" + (f" – {note}" if note else ""), new,
                               {"source": svc, "detail": "via streamrip", "url": url})
                return
            except (RuntimeError, tagger.VerificationFailed) as e:
                last_err = str(e)
                db.log("warning", f"streamrip failed for {label(r)} ({url}): {e}")
        shutil.rmtree(_staging(r), ignore_errors=True)
        await next_source(r, f"streamrip: {last_err[:150]}")


# ---- yt-dlp -----------------------------------------------------------------

def _staging(r: dict) -> str:
    return os.path.join(config.get("staging_path"), f"request-{r['id']}")


async def start_ytdlp(r: dict, pick_url: str | None = None) -> None:
    async with loop_local("ytdlp", lambda: asyncio.Semaphore(2)):
        _check(r["id"])
        out = _staging(r)
        if r["kind"] == "track":
            upd(r, status="searching", message="Searching YouTube Music…")
            if pick_url:
                hit = {"videoId": None} if "music.youtube.com" in pick_url else None
                url = pick_url
            else:
                hit = await ytmusic.find_song(r["artist"], r["title"], r["detail"].get("length"))
                url = f"https://music.youtube.com/watch?v={hit['videoId']}" if hit else await ytdlp.find_best(r["artist"], r["title"], r["detail"].get("length"))
            if not url:
                await next_source(r, "no match on YouTube")
                return
            if hit and hit.get("cover") and not r.get("cover_url"):
                tagger.COVER_HINT.set(hit["cover"])
            upd(r, status="downloading", message=f"Downloading {url}")
            res = await ytdlp.download(url, out, _progress_hook(r))
            if not res["files"]:
                await next_source(r, "yt-dlp produced no audio")
                return
            try:
                new, note = await tagger.import_single(res["files"][0], r["artist"], r["title"], r["mbid"], out, r["detail"].get("length"))
            except tagger.VerificationFailed as e:
                shutil.rmtree(out, ignore_errors=True)
                await next_source(r, f"YouTube match was the wrong audio ({e})")
                return
            shutil.rmtree(out, ignore_errors=True)
            await complete(r, ("Downloaded from YouTube Music" if hit else "Downloaded from YouTube") + (f" – {note}" if note else ""), [new],
                           {"source": "YouTube Music" if hit else "YouTube", "url": url})
            return
        rel = await _release_info(r)
        album_title = r["album"] or r["title"]
        # whole official album from YouTube Music in one go
        upd(r, status="searching", message="Searching YouTube Music for the album…")
        ext = r["detail"].get("ext") or {}
        if pick_url:
            yt_album = {"title": r["detail"]["pick"].get("title") or album_title, "playlist_url": pick_url}
        elif ext.get("ytm") or ext.get("bandcamp"):
            # release MusicBrainz lacks: download exactly the YouTube Music / Bandcamp release it was found on
            yt_album = {"title": album_title, "playlist_url": ext.get("bandcamp")}
            if ext.get("ytm"):
                try:
                    pl = (await ytmusic.run(ytmusic.anon().get_album, ext["ytm"])).get("audioPlaylistId")
                    yt_album["playlist_url"] = f"https://music.youtube.com/playlist?list={pl}" if pl else yt_album["playlist_url"]
                except Exception as e:  # noqa: BLE001
                    db.log("warning", f"YouTube Music album lookup failed for {label(r)}: {e}")
        else:
            yt_album = await ytmusic.find_album(r["artist"], album_title, len(rel["tracks"]) if rel else None)
        if yt_album and yt_album["playlist_url"]:
            upd(r, status="downloading", message=f"YouTube Music album: {yt_album['title']}")
            try:
                res = await ytdlp.download(yt_album["playlist_url"], os.path.join(out, "album"), _progress_hook(r), playlist=True)
                if res["files"] and (pick_url or not rel or len(res["files"]) >= len(rel["tracks"]) - max(1, len(rel["tracks"]) // 5)):
                    via = "Bandcamp" if "bandcamp.com" in yt_album["playlist_url"] else "YouTube Music"
                    upd(r, status="importing", progress=100, message="Verifying, tagging and importing…")
                    new = await tagger.import_album_files(res["files"], r["artist"], album_title, r["mbid"], out, source_meta=_source_meta(r))
                    shutil.rmtree(out, ignore_errors=True)
                    await complete(r, f"Downloaded from {via}", new,
                                   {"source": via, "detail": f"album {yt_album['title']}", "url": yt_album["playlist_url"]})
                    return
            except tagger.VerificationFailed as e:
                db.log("warning", f"YouTube Music album for {label(r)} rejected: {e}")
            except Exception as e:  # noqa: BLE001
                db.log("warning", f"YouTube Music album download failed for {label(r)}: {e}")
            shutil.rmtree(out, ignore_errors=True)
            if pick_url:
                await next_source(r, "the chosen YouTube album could not be downloaded or didn't match")
                return
        if not rel:
            await next_source(r, "no MusicBrainz tracklist for this album")
            return
        files, missing = [], []
        n = len(rel["tracks"])
        for i, t in enumerate(rel["tracks"]):
            _check(r["id"])
            upd(r, status="downloading", progress=round(i / n * 100, 1), message=f"YouTube: track {i + 1}/{n} – {t['title']}")
            try:
                hit = await ytmusic.find_song(t["artist"] or r["artist"], t["title"], t["length"])
                url = f"https://music.youtube.com/watch?v={hit['videoId']}" if hit else await ytdlp.find_best(t["artist"] or r["artist"], t["title"], t["length"])
                res = await ytdlp.download(url, os.path.join(out, f"{i + 1:02d}")) if url else {"files": []}
                files += res["files"]
                if not res["files"]:
                    missing.append(t["title"])
            except Exception as e:  # noqa: BLE001
                missing.append(t["title"])
                db.log("warning", f"yt-dlp failed for {t['title']}: {e}")
        if len(missing) > max(1, n // 5):
            shutil.rmtree(out, ignore_errors=True)
            await next_source(r, f"YouTube missing {len(missing)}/{n} tracks")
            return
        upd(r, status="importing", progress=100, message="Verifying, tagging and importing…")
        try:
            new = await tagger.import_album_files(files, r["artist"], r["album"] or r["title"], r["mbid"], out, source_meta=_source_meta(r))
        except tagger.VerificationFailed as e:
            shutil.rmtree(out, ignore_errors=True)
            await next_source(r, f"YouTube matches were wrong ({e})")
            return
        shutil.rmtree(out, ignore_errors=True)
        await complete(r, "Downloaded from YouTube" + (f" ({len(missing)} track(s) missing)" if missing else ""), new,
                       {"source": "YouTube Music", "detail": "matched track by track"})


def _progress_hook(r: dict):
    last = {"t": 0.0}

    def hook(d):
        if d.get("status") == "downloading" and time.time() - last["t"] > 2:
            last["t"] = time.time()
            total = d.get("total_bytes") or d.get("total_bytes_estimate") or 0
            pct = (d.get("downloaded_bytes", 0) / total * 100) if total else 0
            db.update_request(r["id"], progress=round(pct, 1))
    return hook


async def run_url(r: dict) -> None:
    async with loop_local("ytdlp", lambda: asyncio.Semaphore(2)):
        out = _staging(r)
        res = await ytdlp.download(r["url"], out, _progress_hook(r), playlist=True)
        _check(r["id"])
        if not res["files"]:
            await fail(r, "yt-dlp produced no audio files")
            return
        upd(r, status="importing", message="Tagging and importing…")
        entries = res["entries"]
        if len(entries) == 1 and entries[0].get("thumbnail") and not r.get("cover_url"):
            tagger.COVER_HINT.set(entries[0]["thumbnail"])
        albums = {e.get("album") for e in entries if e.get("album")}
        new = []
        if len(res["files"]) > 1 and len(albums) == 1:
            artist = entries[0].get("artist") or r["artist"] or "Unknown Artist"
            new = await tagger.import_album_files(res["files"], artist, albums.pop(), None, out, reject=False)
        else:
            for path in res["files"]:
                t = library.read_tags(path)
                artist, title = t["artist"], t["title"]
                base = os.path.splitext(os.path.basename(path))[0]
                if (not artist or artist == "Unknown Artist") and " - " in base:
                    artist, title = base.split(" - ", 1)
                if " - " in title and artist and title.lower().startswith(artist.lower() + " - "):
                    title = title.split(" - ", 1)[1]
                try:
                    p, _ = await tagger.import_single(path, artist or "Unknown Artist", title, None, out, reject=False)
                    new.append(p)
                except tagger.VerificationFailed as e:
                    db.log("warning", f"Skipped {os.path.basename(path)}: {e}")
        shutil.rmtree(out, ignore_errors=True)
        from urllib.parse import urlsplit
        host = urlsplit(r["url"]).hostname or "link"
        await complete(r, f"Downloaded {len(new)} file(s)", new,
                       {"source": "YouTube Music" if "music.youtube" in host else host.removeprefix("www."), "detail": "direct link", "url": r["url"]})


async def run_streamrip_url(r: dict) -> None:
    async with loop_local("streamrip", lambda: asyncio.Semaphore(1)):
        out = _staging(r)
        files = await streamrip.download(r["url"], out)
        upd(r, status="importing", message="Tagging and importing…")
        groups: dict[str, list] = {}
        for f in files:
            groups.setdefault(os.path.dirname(f), []).append(f)
        new = []
        for folder, fs in groups.items():
            t = library.read_tags(fs[0])
            if len(fs) > 1:
                new += await tagger.import_album_files(fs, t["albumartist"], t["album"] or "", None, out, reject=False)
            else:
                p, _ = await tagger.import_single(fs[0], t["artist"], t["title"], None, out, reject=False)
                new.append(p)
        shutil.rmtree(out, ignore_errors=True)
        svc = "Deezer" if "deezer" in r["url"] else "Tidal" if "tidal" in r["url"] else "Qobuz"
        await complete(r, f"Downloaded {len(new)} file(s) via streamrip", new, {"source": svc, "detail": "via streamrip", "url": r["url"]})


# ---- loop -------------------------------------------------------------------

async def step(rid: int) -> None:
    r = db.get_request(rid)
    if not r or r["status"] not in ACTIVE:
        return
    tagger.COVER_HINT.set(r.get("cover_url"))
    try:
        if r["status"] == "queued":
            await start(r)
        elif r["source"] == "lidarr" and r["status"] in ("monitoring", "downloading"):
            await poll_lidarr(r)
        elif r["source"] in ("slskd", "slskd_lossless") and r["status"] == "downloading":
            await poll_slskd(r)
        elif r["source"] == "album" and r["status"] == "monitoring":
            await poll_album(r)
        elif r["status"] in ("searching", "importing"):
            pass  # handled inside a running task; stale states are reset on startup
    except Cancelled:
        pass
    except Exception as e:  # noqa: BLE001
        traceback.print_exc()
        r = db.get_request(rid)
        if r and r["status"] in ACTIVE:
            if r["source"] and r["detail"].get("sources") is not None:
                await next_source(r, f"error: {e}")
            else:
                await fail(r, str(e))


def recover() -> None:
    """Reset requests that were mid-task when the app stopped."""
    for r in db.query("SELECT id, status, source FROM requests WHERE status IN ('searching','importing','downloading')"):
        if r["status"] == "downloading" and r["source"] in ("slskd", "slskd_lossless", "lidarr"):
            continue
        db.update_request(r["id"], status="queued", message="Restarted after app restart", detail="{}")


async def loop() -> None:
    recover()
    last_scan = 0.0
    last_sync = time.time() - 60 * 60  # first sync shortly after start
    last_upgrade = time.time()
    last_follow = time.time() - 23 * 3600  # first new-release check shortly after start
    last_daily = time.time() - 23 * 3600
    last_discover = time.time() - 5 * 3600
    while not _stop.is_set():
        try:
            for r in db.query(f"SELECT id FROM requests WHERE status IN ({','.join('?' * len(ACTIVE))}) ORDER BY id", ACTIVE):
                t = _running.get(r["id"])
                if t and not t.done():
                    continue
                _running[r["id"]] = asyncio.create_task(step(r["id"]))
            for k in [k for k, t in _running.items() if t.done()]:
                _running.pop(k, None)
            interval = int(config.get("scan_interval_minutes")) * 60
            if interval and time.time() - last_scan > interval and not library.state["scanning"]:
                last_scan = time.time()
                asyncio.create_task(library.scan())
            if config.get("upgrade_lossy") and time.time() - last_upgrade > 6 * 3600:
                last_upgrade = time.time()
                asyncio.create_task(queue_lossy_upgrades())
            follow_every = int(config.get("follow_check_hours")) * 3600
            if follow_every and time.time() - last_follow > follow_every:
                last_follow = time.time()
                from .services import follows
                asyncio.create_task(follows.check_all())
            if time.time() - last_daily > 24 * 3600:
                last_daily = time.time()
                asyncio.create_task(retag_external())
            if config.get("discover_auto_count") and time.time() - last_discover > 6 * 3600:
                last_discover = time.time()
                from .services import discovery
                asyncio.create_task(discovery.auto_discover())
            sync_every = int(config.get("sync_interval_minutes")) * 60
            if sync_every and time.time() - last_sync > sync_every:
                last_sync = time.time()
                from .services import sync
                asyncio.create_task(sync.sync_all())
            db.execute("DELETE FROM sessions WHERE expires_at < ?", (time.time(),))
        except Exception:  # noqa: BLE001
            traceback.print_exc()
        try:
            await asyncio.wait_for(_stop.wait(), timeout=5)
        except asyncio.TimeoutError:
            pass


async def cancel(rid: int) -> None:
    r = db.get_request(rid)
    if not r:
        return
    st = r["detail"].get("slskd")
    if r["source"] in ("slskd", "slskd_lossless") and st and st.get("idx", -1) >= 0:
        c = st["candidates"][st["idx"]]
        for f in await slskd.user_downloads(c["username"]):
            if f.get("filename") in {x["filename"] for x in c["files"]}:
                await slskd.cancel(c["username"], f["id"])
    t = _running.pop(rid, None)
    if t and not t.done():
        t.cancel()
