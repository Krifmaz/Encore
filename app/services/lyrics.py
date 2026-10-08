"""Lyrics from LRCLIB (free, no key): saved next to each song as .lrc (time-synced) or .txt (plain),
which Jellyfin, Navidrome, Plexamp and most players pick up too."""
import asyncio
import os
import re

from .. import __version__, config, db
from .http import client

API = "https://lrclib.net/api"
HEADERS = {"User-Agent": f"Encore/{__version__} (https://github.com/Krifmaz/Encore)"}


def sidecar(path: str) -> tuple[str | None, bool]:
    """(lyrics text, synced?) from an existing .lrc/.txt next to the file."""
    base = os.path.splitext(path)[0]
    for ext, synced in ((".lrc", True), (".txt", False)):
        try:
            with open(base + ext, encoding="utf-8") as fh:
                return fh.read(), synced
        except OSError:
            continue
    return None, False


def _clean(s: str) -> str:
    return re.sub(r"\s*[\(\[](feat|ft|with|remaster)[^\)\]]*[\)\]]", "", s or "", flags=re.I).strip()


async def lookup(artist: str, title: str, album: str = "", duration: float | None = None) -> dict | None:
    params = {"artist_name": artist, "track_name": _clean(title)}
    if album:
        params["album_name"] = album
    if duration:
        params["duration"] = int(round(duration))
    r = await client().get(f"{API}/get", params=params, headers=HEADERS)
    if r.status_code == 200 and (r.json().get("syncedLyrics") or r.json().get("plainLyrics") or r.json().get("instrumental")):
        return r.json()
    r = await client().get(f"{API}/search", params={"artist_name": artist, "track_name": _clean(title)}, headers=HEADERS)
    hits = r.json() if r.status_code == 200 and isinstance(r.json(), list) else []
    if duration:
        hits = [h for h in hits if not h.get("duration") or abs(h["duration"] - duration) <= 3]
    hits.sort(key=lambda h: (not h.get("syncedLyrics"), not h.get("plainLyrics")))
    return hits[0] if hits else None


async def fetch_for_track(t: dict, force: bool = False) -> dict:
    """Return {lyrics, synced, source}; downloads and saves them when missing."""
    text, synced = sidecar(t["path"])
    if text and not force:
        return {"lyrics": text, "synced": synced, "source": "file"}
    hit = await lookup(t["artist"], t["title"], t.get("album") or "", t.get("duration"))
    if not hit:
        return {"lyrics": None, "synced": False, "source": None}
    if hit.get("instrumental"):
        return {"lyrics": "", "synced": False, "source": "LRCLIB", "instrumental": True}
    synced = bool(hit.get("syncedLyrics"))
    text = hit.get("syncedLyrics") or hit.get("plainLyrics") or ""
    if config.get("lyrics_save_files"):
        base = os.path.splitext(t["path"])[0]
        try:
            with open(base + (".lrc" if synced else ".txt"), "w", encoding="utf-8") as fh:
                fh.write(text)
            if synced and hit.get("plainLyrics") and config.get("lyrics_embed"):
                await asyncio.to_thread(_embed, t["path"], hit["plainLyrics"])
        except OSError as e:
            db.log("warning", f"Could not save lyrics for {t['title']}: {e}")
    elif hit.get("plainLyrics") and config.get("lyrics_embed"):
        await asyncio.to_thread(_embed, t["path"], hit["plainLyrics"])
    return {"lyrics": text, "synced": synced, "source": "LRCLIB"}


def _embed(path: str, plain: str) -> None:
    """Unsynced lyrics inside the file (USLT / LYRICS), for players that don't read .lrc files."""
    import mutagen
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".mp3":
            from mutagen.id3 import ID3, USLT, ID3NoHeaderError
            try:
                tags = ID3(path)
            except ID3NoHeaderError:
                tags = ID3()
            tags.delall("USLT")
            tags.add(USLT(encoding=3, lang="eng", desc="", text=plain))
            tags.save(path)
        elif ext in (".m4a", ".mp4", ".alac", ".aac"):
            f = mutagen.File(path)
            f["\xa9lyr"] = [plain]
            f.save()
        else:
            f = mutagen.File(path)
            if f is not None:
                f["LYRICS"] = [plain]
                f.save()
    except Exception as e:  # noqa: BLE001
        db.log("warning", f"Could not embed lyrics in {os.path.basename(path)}: {e}")


async def fetch_missing(track_ids: list[int] | None = None, limit: int = 200) -> dict:
    """Fetch lyrics for songs that have none yet (used after imports and by the scheduled task)."""
    if track_ids:
        rows = db.query(f"SELECT id, path, artist, title, album, duration FROM tracks WHERE id IN ({','.join('?' * len(track_ids))})", track_ids)
    else:
        rows = db.query("SELECT id, path, artist, title, album, duration FROM tracks ORDER BY added_at DESC LIMIT ?", (limit * 5,))
    done = found = 0
    for t in rows:
        if sidecar(t["path"])[0] is not None:
            continue
        if done >= limit:
            break
        done += 1
        try:
            if (await fetch_for_track(t))["lyrics"]:
                found += 1
        except Exception:  # noqa: BLE001
            pass
        await asyncio.sleep(0.2)   # be gentle with the free service
    return {"checked": done, "found": found}
