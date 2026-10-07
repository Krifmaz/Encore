"""Chromaprint fingerprints + AcoustID lookups: identify what a file *actually* contains."""
import asyncio
import json
import shutil
import time

from .. import config
from .http import client

API = "https://api.acoustid.org/v2/lookup"
_lock = asyncio.Lock()
_last = 0.0


def available() -> bool:
    return bool(shutil.which("fpcalc") and config.get("acoustid_api_key"))


async def fingerprint(path: str) -> tuple[float, str] | None:
    if not shutil.which("fpcalc"):
        return None
    proc = await asyncio.create_subprocess_exec(
        "fpcalc", "-json", "-length", "120", path,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    out, _ = await proc.communicate()
    if proc.returncode != 0:
        return None
    data = json.loads(out)
    return float(data["duration"]), data["fingerprint"]


async def lookup(duration: float, fp: str) -> list[dict]:
    """Returns [{acoustid, score, recordings:[{id,title,artist,length,release_groups:[...]}]}]."""
    global _last
    async with _lock:  # AcoustID allows 3 requests/second
        wait = 0.34 - (time.time() - _last)
        if wait > 0:
            await asyncio.sleep(wait)
        _last = time.time()
    r = await client().post(API, data={
        "client": config.get("acoustid_api_key"), "duration": int(duration), "fingerprint": fp,
        "meta": "recordings releasegroups compress",
    })
    data = r.json()
    if data.get("status") != "ok":
        raise RuntimeError((data.get("error") or {}).get("message", "AcoustID error"))
    out = []
    for res in data.get("results", []):
        recs = []
        for rec in res.get("recordings", []) or []:
            recs.append({
                "id": rec.get("id"),
                "title": rec.get("title", ""),
                "artist": "".join(a.get("name", "") + (a.get("joinphrase") or "") for a in rec.get("artists", []) or []),
                "length": rec.get("duration"),
                "release_groups": [g.get("title") for g in rec.get("releasegroups", []) or []][:3],
            })
        out.append({"acoustid": res.get("id"), "score": res.get("score", 0), "recordings": recs})
    out.sort(key=lambda x: -x["score"])
    return out


async def identify(path: str) -> dict | None:
    fp = await fingerprint(path)
    if not fp:
        return None
    results = await lookup(*fp)
    return {"duration": fp[0], "results": results}
