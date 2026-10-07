"""Request pipeline: Lidarr → slskd → yt-dlp, with import + tagging."""
import asyncio
import os
import re
import shutil
import time
import traceback

from . import config, db
from .services import library, lidarr, musicbrainz as mb, notify, slskd, streamrip, tagger, ytdlp

ACTIVE = ("queued", "searching", "downloading", "monitoring", "importing")
_running: dict[int, asyncio.Task] = {}
_yt_sem = asyncio.Semaphore(2)
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

async def complete(r: dict, message: str, new_paths: list[str] | None = None) -> None:
    if new_paths:
        await library.index_paths(new_paths)
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
    # already have it?
    if r["kind"] == "album" and (library.has_release_group(r["mbid"]) or library.has_album(r["artist"], r["album"] or r["title"])):
        await complete(r, "Already in library")
        return
    if r["kind"] == "track" and library.has_track(r["mbid"], r["artist"], r["title"]):
        await complete(r, "Already in library")
        return
    if r["kind"] == "artist":
        if lidarr.enabled():
            upd(r, source="lidarr", status="searching", message="Adding artist to Lidarr…")
            a = await lidarr.ensure_artist(r["mbid"], monitor_all=True)
            await complete(r, f"Added to Lidarr – monitoring all albums ({a.get('artistName', r['artist'])})")
        else:
            await expand_artist(r)
        return
    order = [s.strip() for s in config.get("source_order").split(",") if s.strip()]
    avail = {"lidarr": lidarr.enabled() and r["kind"] == "album" and r["mbid"], "slskd": slskd.enabled(),
             "ytdlp": ytdlp.enabled(), "streamrip": streamrip.enabled()}
    d["sources"] = [s for s in order if avail.get(s)]
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

async def _release_info(r: dict) -> dict | None:
    d = r["detail"]
    if "release" not in d and r["kind"] == "album" and r["mbid"]:
        try:
            rid = await mb.best_release_for_group(r["mbid"])
            rel = await mb.release(rid) if rid else None
            d["release"] = {"id": rel["id"], "tracks": [{"title": t["title"], "artist": t["artist"], "length": t["length"], "recording_id": t["recording_id"]} for t in rel["tracks"]]} if rel else None
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"MusicBrainz release lookup failed: {e}")
            d["release"] = None
    return d.get("release")


def _q(*parts: str) -> str:
    import re
    s = " ".join(p for p in parts if p)
    s = re.sub(r"[\(\[].*?[\)\]]", " ", s)
    s = re.sub(r"[^\w\s']", " ", s)
    return " ".join(s.split())


async def start_slskd(r: dict) -> None:
    upd(r, status="searching", message="Searching Soulseek…")
    if r["kind"] == "album":
        rel = await _release_info(r)
        n = len(rel["tracks"]) if rel else None
        album = r["album"] or r["title"]
        cands = []
        for query in (_q(r["artist"], album), _q(album)):
            responses = await slskd.search(query)
            cands = slskd.rank_album(responses, r["artist"], album, n)
            if cands:
                break
    else:
        length = r["detail"].get("length")
        responses = await slskd.search(_q(r["artist"], r["title"]))
        cands = slskd.rank_track(responses, r["artist"], r["title"], length)
    _check(r["id"])
    if not cands:
        await next_source(r, "no suitable results on Soulseek")
        return
    r["detail"]["slskd"] = {"candidates": cands, "idx": -1}
    await _slskd_try_next(r)


async def _slskd_try_next(r: dict, reason: str = "") -> None:
    st = r["detail"]["slskd"]
    if st["idx"] >= 0:
        c = st["candidates"][st["idx"]]
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
    transfers = [f for f in await slskd.user_downloads(c["username"]) if f.get("filename") in wanted]
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
            new = await tagger.import_album_files(paths, r["artist"], r["album"] or r["title"], r["mbid"], root)
        else:
            p, note = await tagger.import_single(paths[0], r["artist"], r["title"], r["mbid"], root, r["detail"].get("length"))
            new = [p]
    except tagger.VerificationFailed as e:
        _discard(paths, root)
        db.log("warning", f"Rejected download from {c['username']} for {label(r)}: {e}")
        upd(r, status="downloading", message=f"Rejected {c['username']}'s files: {e}")
        await _slskd_try_next(r, f"wrong audio ({e})")
        return
    await complete(r, f"Downloaded from Soulseek ({c['format'].upper()})" + (f" – {note}" if note else ""), new)


def _discard(paths: list[str], root: str) -> None:
    for p in paths:
        try:
            os.remove(p)
        except OSError:
            pass
        tagger.prune_empty(os.path.dirname(p), root)


# ---- streamrip (Qobuz / Tidal / Deezer) ---------------------------------------

_rip_sem = asyncio.Semaphore(1)


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


async def start_streamrip(r: dict) -> None:
    async with _rip_sem:
        _check(r["id"])
        upd(r, status="searching", message="Looking for an exact lossless match…")
        if r["kind"] == "album":
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
                    new = await tagger.import_album_files(files, r["artist"], r["album"] or r["title"], r["mbid"], out)
                    note = ""
                else:
                    p, note = await tagger.import_single(files[0], r["artist"], r["title"], r["mbid"], out, r["detail"].get("length"))
                    new = [p]
                shutil.rmtree(_staging(r), ignore_errors=True)
                svc = "Deezer" if "deezer" in url else "Tidal" if "tidal" in url else "Qobuz"
                await complete(r, f"Downloaded from {svc} via streamrip" + (f" – {note}" if note else ""), new)
                return
            except (RuntimeError, tagger.VerificationFailed) as e:
                last_err = str(e)
                db.log("warning", f"streamrip failed for {label(r)} ({url}): {e}")
        shutil.rmtree(_staging(r), ignore_errors=True)
        await next_source(r, f"streamrip: {last_err[:150]}")


# ---- yt-dlp -----------------------------------------------------------------

def _staging(r: dict) -> str:
    return os.path.join(config.get("staging_path"), f"request-{r['id']}")


async def start_ytdlp(r: dict) -> None:
    async with _yt_sem:
        _check(r["id"])
        out = _staging(r)
        if r["kind"] == "track":
            upd(r, status="searching", message="Searching YouTube…")
            url = await ytdlp.find_best(r["artist"], r["title"], r["detail"].get("length"))
            if not url:
                await next_source(r, "no match on YouTube")
                return
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
            await complete(r, "Downloaded from YouTube" + (f" – {note}" if note else ""), [new])
            return
        rel = await _release_info(r)
        if not rel:
            await next_source(r, "no MusicBrainz tracklist for this album")
            return
        files, missing = [], []
        n = len(rel["tracks"])
        for i, t in enumerate(rel["tracks"]):
            _check(r["id"])
            upd(r, status="downloading", progress=round(i / n * 100, 1), message=f"YouTube: track {i + 1}/{n} – {t['title']}")
            try:
                url = await ytdlp.find_best(t["artist"] or r["artist"], t["title"], t["length"])
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
            new = await tagger.import_album_files(files, r["artist"], r["album"] or r["title"], r["mbid"], out)
        except tagger.VerificationFailed as e:
            shutil.rmtree(out, ignore_errors=True)
            await next_source(r, f"YouTube matches were wrong ({e})")
            return
        shutil.rmtree(out, ignore_errors=True)
        await complete(r, "Downloaded from YouTube" + (f" ({len(missing)} track(s) missing)" if missing else ""), new)


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
    async with _yt_sem:
        out = _staging(r)
        res = await ytdlp.download(r["url"], out, _progress_hook(r), playlist=True)
        _check(r["id"])
        if not res["files"]:
            await fail(r, "yt-dlp produced no audio files")
            return
        upd(r, status="importing", message="Tagging and importing…")
        entries = res["entries"]
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
        await complete(r, f"Downloaded {len(new)} file(s)", new)


async def run_streamrip_url(r: dict) -> None:
    async with _rip_sem:
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
        await complete(r, f"Downloaded {len(new)} file(s) via streamrip", new)


# ---- loop -------------------------------------------------------------------

async def step(rid: int) -> None:
    r = db.get_request(rid)
    if not r or r["status"] not in ACTIVE:
        return
    try:
        if r["status"] == "queued":
            await start(r)
        elif r["source"] == "lidarr" and r["status"] in ("monitoring", "downloading"):
            await poll_lidarr(r)
        elif r["source"] == "slskd" and r["status"] == "downloading":
            await poll_slskd(r)
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
        if r["status"] == "downloading" and r["source"] in ("slskd", "lidarr"):
            continue
        db.update_request(r["id"], status="queued", message="Restarted after app restart", detail="{}")


async def loop() -> None:
    recover()
    last_scan = 0.0
    last_sync = time.time() - 60 * 60  # first sync shortly after start
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
    if r["source"] == "slskd" and st and st.get("idx", -1) >= 0:
        c = st["candidates"][st["idx"]]
        for f in await slskd.user_downloads(c["username"]):
            if f.get("filename") in {x["filename"] for x in c["files"]}:
                await slskd.cancel(c["username"], f["id"])
    t = _running.pop(rid, None)
    if t and not t.done():
        t.cancel()
