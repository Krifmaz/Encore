"""slskd (Soulseek) API client + result ranking."""
import asyncio
import os
import re
import uuid
from difflib import SequenceMatcher

from .. import config
from .http import client

AUDIO_EXT = {"flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff", "wma", "ape", "wv"}
LOSSLESS = {"flac", "wav", "alac", "aiff", "ape", "wv"}
DONE_OK = "Succeeded"
DONE_BAD = ("Errored", "Rejected", "TimedOut", "Cancelled", "Aborted", "Failed")


class SlskdError(Exception):
    pass


def enabled() -> bool:
    return bool(config.get("slskd_enabled") and config.get("slskd_url") and config.get("slskd_api_key"))


async def _req(method: str, path: str, **kw):
    url = config.get("slskd_url").rstrip("/") + "/api/v0/" + path.lstrip("/")
    r = await client().request(method, url, headers={"X-API-Key": config.get("slskd_api_key")}, **kw)
    if r.status_code >= 400:
        raise SlskdError(f"slskd {method} {path} → {r.status_code}: {r.text[:300]}")
    try:
        return r.json() if r.content else None
    except ValueError:
        return None


async def status() -> dict:
    return await _req("GET", "application")


async def search(text: str, timeout_s: int | None = None) -> list[dict]:
    """Run a search and wait for it to finish. Returns responses."""
    timeout_s = timeout_s or int(config.get("slskd_search_timeout"))
    sid = str(uuid.uuid4())
    await _req("POST", "searches", json={
        "id": sid,
        "searchText": text,
        "searchTimeout": timeout_s * 1000,
        "responseLimit": 200,
        "fileLimit": 20000,
        "filterResponses": True,
        "minimumResponseFileCount": 1,
    })
    try:
        for _ in range(timeout_s + 20):
            await asyncio.sleep(1)
            st = await _req("GET", f"searches/{sid}")
            if st and st.get("isComplete"):
                break
        return await _req("GET", f"searches/{sid}/responses") or []
    finally:
        try:
            await _req("DELETE", f"searches/{sid}")
        except Exception:
            pass


async def enqueue(username: str, files: list[dict]) -> None:
    await _req("POST", f"transfers/downloads/{username}", json=[{"filename": f["filename"], "size": f["size"]} for f in files])


async def user_downloads(username: str) -> list[dict]:
    try:
        data = await _req("GET", f"transfers/downloads/{username}")
    except SlskdError:
        return []
    files = []
    for d in (data or {}).get("directories", []):
        files.extend(d.get("files", []))
    return files


async def all_downloads() -> list[dict]:
    data = await _req("GET", "transfers/downloads") or []
    out = []
    for u in data:
        for d in u.get("directories", []):
            for f in d.get("files", []):
                out.append({
                    "id": f.get("id"),
                    "username": u.get("username"),
                    "filename": f.get("filename", "").replace("\\", "/").split("/")[-1],
                    "directory": d.get("directory", "").replace("\\", "/").split("/")[-1],
                    "state": f.get("state", ""),
                    "progress": round(f.get("percentComplete") or 0, 1),
                    "size": f.get("size", 0),
                    "speed": f.get("averageSpeed", 0),
                })
    return out


async def cancel(username: str, file_id: str, remove: bool = True) -> None:
    try:
        await _req("DELETE", f"transfers/downloads/{username}/{file_id}", params={"remove": str(remove).lower()})
    except SlskdError:
        pass


async def clear_completed() -> None:
    await _req("DELETE", "transfers/downloads/all/completed")


# ---- ranking ----------------------------------------------------------------

def _norm(s: str) -> str:
    s = (s or "").lower()
    s = re.sub(r"[\(\[].*?[\)\]]", " ", s)
    s = re.sub(r"[^a-z0-9]+", " ", s)
    return " ".join(s.split())


def _sim(a: str, b: str) -> float:
    return SequenceMatcher(None, _norm(a), _norm(b)).ratio()


def _ext(fn: str) -> str:
    return fn.rsplit(".", 1)[-1].lower() if "." in fn else ""


def _contains(hay: str, needle: str) -> float:
    words = _norm(needle).split()
    if not words:
        return 0.0
    hay_n = " " + _norm(hay) + " "
    return sum(1 for w in words if f" {w} " in hay_n) / len(words)


def _format_score(files: list[dict], lossless_only: bool = False) -> float | None:
    pref = config.get("slskd_preferred_format")
    min_br = int(config.get("slskd_min_bitrate"))
    exts = {_ext(f["filename"]) for f in files}
    if len(exts) > 1:
        return None
    ext = exts.pop()
    if lossless_only and ext not in LOSSLESS:
        return None
    if ext in LOSSLESS:
        hires = all((f.get("bitDepth") or 16) >= 24 for f in files)
        return {"flac": 30, "mp3": 8, "any": 25}.get(pref, 25) + (6 if hires and pref != "mp3" else 0)
    br = min((f.get("bitRate") or 0) for f in files)
    if br and br < min_br:
        return None
    base = {"flac": 10, "mp3": 30, "any": 22}.get(pref, 22) if ext == "mp3" else 12
    if br >= 320:
        base += 5
    elif br and br < 256:
        base -= 6
    return base


def rank_album(responses: list[dict], artist: str, album: str, track_count: int | None, lossless_only: bool = False) -> list[dict]:
    candidates = []
    for resp in responses:
        dirs: dict[str, list] = {}
        for f in resp.get("files", []):
            if _ext(f["filename"]) not in AUDIO_EXT:
                continue
            d = f["filename"].replace("\\", "/").rsplit("/", 1)[0]
            dirs.setdefault(d, []).append(f)
        for d, files in dirs.items():
            fmt = _format_score(files, lossless_only)
            if fmt is None:
                continue
            n = len(files)
            score = fmt
            if track_count:
                if n < track_count:
                    continue
                if n > track_count * 2 + 2:
                    continue
                score += 25 if n == track_count else max(0, 15 - (n - track_count) * 3)
            path_tail = "/".join(d.split("/")[-2:])
            score += _contains(path_tail, album) * 30
            score += _contains(d, artist) * 10
            if re.search(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore)\b", path_tail) and not re.search(
                r"(?i)\b(live|remix|karaoke|instrumental)\b", album
            ):
                score -= 15
            if resp.get("hasFreeUploadSlot"):
                score += 8
            score -= min(resp.get("queueLength", 0), 50) / 5
            score += min((resp.get("uploadSpeed") or 0) / 1_000_000, 10)
            if _contains(path_tail, album) < 0.5:
                continue
            candidates.append({
                "username": resp["username"],
                "directory": d,
                "files": [{"filename": f["filename"], "size": f["size"]} for f in sorted(files, key=lambda x: x["filename"])],
                "format": _ext(files[0]["filename"]),
                "bitrate": min((f.get("bitRate") or 0) for f in files),
                "score": round(score, 1),
            })
    candidates.sort(key=lambda c: -c["score"])
    seen, unique = set(), []
    for c in candidates:
        if c["username"] in seen:
            continue
        seen.add(c["username"])
        unique.append(c)
    return unique[: int(config.get("slskd_max_candidates"))]


def rank_track(responses: list[dict], artist: str, title: str, length: float | None, lossless_only: bool = False) -> list[dict]:
    candidates = []
    for resp in responses:
        for f in resp.get("files", []):
            if _ext(f["filename"]) not in AUDIO_EXT:
                continue
            fmt = _format_score([f], lossless_only)
            if fmt is None:
                continue
            name = f["filename"].replace("\\", "/")
            base = name.rsplit("/", 1)[-1]
            t = _contains(base, title)
            if t < 0.6:
                continue
            score = fmt + t * 40 + _contains(name, artist) * 15
            if re.search(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore|cover)\b", base) and not re.search(
                r"(?i)\b(live|remix|instrumental)\b", title
            ):
                score -= 20
            if length and f.get("length"):
                diff = abs(f["length"] - length)
                score += 10 if diff < 5 else (-10 if diff > 20 else 0)
            if resp.get("hasFreeUploadSlot"):
                score += 8
            score -= min(resp.get("queueLength", 0), 50) / 5
            candidates.append({
                "username": resp["username"],
                "directory": name.rsplit("/", 1)[0],
                "files": [{"filename": f["filename"], "size": f["size"]}],
                "format": _ext(f["filename"]),
                "bitrate": f.get("bitRate") or 0,
                "score": round(score, 1),
            })
    candidates.sort(key=lambda c: -c["score"])
    return candidates[: int(config.get("slskd_max_candidates"))]


def local_path(remote_filename: str) -> str | None:
    """Locate a finished download on the shared volume."""
    root = config.get("slskd_download_path")
    parts = remote_filename.replace("\\", "/").split("/")
    base = parts[-1]
    parent = parts[-2] if len(parts) > 1 else ""
    for cand in (os.path.join(root, parent, base), os.path.join(root, base)):
        if os.path.isfile(cand):
            return cand
    for dirpath, _, files in os.walk(root):
        if base in files:
            return os.path.join(dirpath, base)
    return None
