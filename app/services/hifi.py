"""TIDAL lossless through a HiFi API endpoint – the backend Monochrome (monochrome.tf) streams from.

Point Encore at your own HiFi API instance (backed by your own TIDAL subscription). Several URLs can be
given, comma-separated, and are tried in order. Tracks come as FLAC (or hi-res via a DASH manifest).
"""
import asyncio
import base64
import json
import os
import re

from .. import config, db
from . import matcher
from .http import client


def enabled() -> bool:
    return bool(config.get("hifi_enabled") and endpoints())


def endpoints() -> list[str]:
    return [u.strip().rstrip("/") for u in (config.get("hifi_api_url") or "").split(",") if u.strip().startswith(("http://", "https://"))]


async def _get(path: str, params: dict | None = None) -> dict:
    last = None
    for base in endpoints():
        try:
            r = await client().get(f"{base}{path}", params=params, timeout=30)
            if r.status_code == 200:
                d = r.json()
                return d.get("data", d) if isinstance(d, dict) else {"items": d}
            last = f"{base} answered {r.status_code}"
        except Exception as e:  # noqa: BLE001 – try the next instance
            last = f"{base}: {e}"
    raise RuntimeError(last or "No HiFi API endpoint configured")


def _items(d) -> list[dict]:
    """Search/album responses nest their lists differently between versions: find the list of objects with an id."""
    if isinstance(d, list):
        out = [x.get("item", x) if isinstance(x, dict) else x for x in d]
        return [x for x in out if isinstance(x, dict) and "id" in x]
    if isinstance(d, dict):
        for k in ("items", "tracks", "albums", "data"):
            if k in d:
                got = _items(d[k])
                if got:
                    return got
    return []


def _artist(x: dict) -> str:
    if x.get("artists"):
        return ", ".join(a.get("name", "") for a in x["artists"] if isinstance(a, dict))
    return (x.get("artist") or {}).get("name", "") if isinstance(x.get("artist"), dict) else str(x.get("artist") or "")


def _title(x: dict) -> str:
    t = x.get("title") or ""
    if x.get("version") and x["version"].lower() not in t.lower():
        t += f" ({x['version']})"
    return t


def cover(uuid: str | None, size: int = 1280) -> str | None:
    return f"https://resources.tidal.com/images/{uuid.replace('-', '/')}/{size}x{size}.jpg" if uuid else None


async def search_tracks(q: str = "", isrc: str | None = None) -> list[dict]:
    d = await _get("/search/", {"i": isrc} if isrc else {"s": q})
    return [{"id": x["id"], "title": _title(x), "artist": _artist(x), "album": (x.get("album") or {}).get("title", ""),
             "duration": x.get("duration"), "isrc": x.get("isrc"), "quality": x.get("audioQuality") or "",
             "cover": cover((x.get("album") or {}).get("cover"), 320)} for x in _items(d)]


async def search_albums(q: str) -> list[dict]:
    d = await _get("/search/", {"al": q})
    return [{"id": x["id"], "title": _title(x), "artist": _artist(x), "tracks": x.get("numberOfTracks"), "date": x.get("releaseDate") or "",
             "quality": x.get("audioQuality") or "", "cover": cover(x.get("cover"), 320)} for x in _items(d)]


async def album_tracks(album_id) -> list[dict]:
    d = await _get("/album/", {"id": album_id})
    return [{"id": x["id"], "title": _title(x), "artist": _artist(x), "track": x.get("trackNumber"), "disc": x.get("volumeNumber") or 1,
             "duration": x.get("duration"), "isrc": x.get("isrc")} for x in _items(d) if x.get("title")]


async def find_track(artist: str, title: str, isrc: str | None, length: float | None) -> dict | None:
    want = matcher.describe(title, artist, length=length, isrc=isrc)
    for res in ([await search_tracks(isrc=isrc)] if isrc else []) + [await search_tracks(f"{artist} {title}")]:
        for t in res:
            if matcher.compare(want, matcher.describe(t["title"], t["artist"], length=t["duration"], isrc=t["isrc"])).match:
                return t
    return None


async def find_album(artist: str, album: str, n: int | None) -> dict | None:
    from .catalog import similar
    for a in await search_albums(f"{artist} {album}"):
        if similar(a["title"], album) >= 0.85 and (not artist or similar(a["artist"], artist) >= 0.6):
            if n and a.get("tracks") and abs(a["tracks"] - n) > max(1, n // 5):
                continue
            return a
    return None


async def download_track(track_id, folder: str, name: str) -> str:
    """Fetch one track's lossless stream into folder/name.<ext>."""
    q = config.get("hifi_quality") or "LOSSLESS"
    d = await _get("/track/", {"id": track_id, "quality": q})
    info = d if isinstance(d, dict) else {}
    if isinstance(d, list):
        info = next((x for x in d if isinstance(x, dict) and "manifest" in x), {})
    manifest = info.get("manifest")
    if not manifest:
        url = info.get("OriginalTrackUrl") or info.get("url")
        if not url:
            raise RuntimeError("no stream in the API response")
        return await _fetch(url, folder, name, "flac")
    raw = base64.b64decode(manifest + "=" * (-len(manifest) % 4)).decode("utf-8", "replace")
    os.makedirs(folder, exist_ok=True)
    if "<MPD" in raw:                          # hi-res: DASH manifest → ffmpeg assembles it
        mpd = os.path.join(folder, f"{name}.mpd")
        with open(mpd, "w") as fh:
            fh.write(raw)
        out = os.path.join(folder, f"{name}.flac")
        proc = await asyncio.create_subprocess_exec("ffmpeg", "-loglevel", "error", "-y", "-protocol_whitelist", "file,http,https,tcp,tls,crypto",
                                                    "-i", mpd, "-map", "0:a", "-c:a", "flac", out,
                                                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        _, err = await proc.communicate()
        os.remove(mpd)
        if proc.returncode:
            raise RuntimeError(f"ffmpeg: {err.decode()[-200:]}")
        return out
    m = json.loads(raw)
    if (m.get("encryptionType") or "NONE") != "NONE":
        raise RuntimeError("this stream is encrypted – choose a lossless quality instead of MQA/Atmos")
    ext = "flac" if "flac" in (m.get("codecs") or m.get("mimeType") or "flac") else "m4a"
    return await _fetch(m["urls"][0], folder, name, ext)


async def _fetch(url: str, folder: str, name: str, ext: str) -> str:
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, f"{re.sub(r'[\\\\/:*?\"<>|]+', '_', name)}.{ext}")
    async with client().stream("GET", url, timeout=600, follow_redirects=True) as r:
        if r.status_code >= 400:
            raise RuntimeError(f"stream answered {r.status_code}")
        with open(path, "wb") as fh:
            async for chunk in r.aiter_bytes(1 << 16):
                fh.write(chunk)
    return path


async def status() -> dict:
    try:
        await search_tracks("test")
        return {"ok": True, "message": f"{len(endpoints())} endpoint(s) answering"}
    except Exception as e:  # noqa: BLE001
        db.log("warning", f"HiFi API test failed: {e}")
        return {"ok": False, "message": str(e)}
