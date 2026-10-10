"""Chromaprint fingerprints + AcoustID lookups: identify what a file *actually* contains."""
import asyncio
import json
import shutil
import time

from .. import config
from .http import client, loop_local

API = "https://api.acoustid.org/v2/lookup"
_last = 0.0


_ffmpeg_cp: bool | None = None


def _ffmpeg_chromaprint() -> bool:
    """ffmpeg builds with --enable-chromaprint can fingerprint without fpcalc."""
    global _ffmpeg_cp
    if _ffmpeg_cp is None:
        import subprocess
        try:
            out = subprocess.run(["ffmpeg", "-hide_banner", "-muxers"], capture_output=True, text=True, timeout=10).stdout
            _ffmpeg_cp = " chromaprint " in out
        except Exception:  # noqa: BLE001
            _ffmpeg_cp = False
    return _ffmpeg_cp


def can_fingerprint() -> bool:
    return bool(shutil.which("fpcalc") or (shutil.which("ffprobe") and _ffmpeg_chromaprint()))


def available() -> bool:
    return bool(config.get("acoustid_api_key") and can_fingerprint())


async def _run(*cmd) -> tuple[int, bytes]:
    proc = await asyncio.create_subprocess_exec(*cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    out, _ = await proc.communicate()
    return proc.returncode, out


async def fingerprint(path: str) -> tuple[float, str] | None:
    if shutil.which("fpcalc"):
        code, out = await _run("fpcalc", "-json", "-length", "120", path)
        if code != 0:
            return None
        data = json.loads(out)
        return float(data["duration"]), data["fingerprint"]
    if not can_fingerprint():
        return None
    code, dur = await _run("ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path)
    code2, fp = await _run("ffmpeg", "-v", "error", "-i", path, "-t", "120", "-f", "chromaprint", "-fp_format", "base64", "-")
    if code or code2 or not fp.strip():
        return None
    return float(dur.strip() or 0), fp.decode().strip()


async def lookup(duration: float, fp: str) -> list[dict]:
    """Returns [{acoustid, score, recordings:[{id,title,artist,length,release_groups:[...]}]}]."""
    global _last
    async with loop_local("acoustid", asyncio.Lock):  # AcoustID allows 3 requests/second
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
                "release_group_ids": [g.get("id") for g in rec.get("releasegroups", []) or [] if g.get("id")],
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


# ---- same recording? (used before replacing a file with a "better" copy) ------------------------

def _raw_fp(path: str, seconds: int = 90) -> list[int] | None:
    """Raw Chromaprint fingerprint (list of 32-bit ints) – no AcoustID account needed."""
    import struct
    import subprocess
    try:
        if shutil.which("fpcalc"):
            out = subprocess.run(["fpcalc", "-raw", "-json", "-length", str(seconds), path], capture_output=True, timeout=120)
            return json.loads(out.stdout)["fingerprint"] if out.returncode == 0 else None
        if shutil.which("ffmpeg") and _ffmpeg_chromaprint():
            out = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-t", str(seconds), "-f", "chromaprint", "-fp_format", "raw", "-"],
                                 capture_output=True, timeout=120)
            b = out.stdout
            return list(struct.unpack(f"<{len(b) // 4}I", b[:len(b) // 4 * 4])) if out.returncode == 0 and b else None
    except Exception:  # noqa: BLE001
        return None
    return None


def similarity(a: list[int], b: list[int]) -> float:
    """0..1 – share of matching fingerprint bits at the best alignment (same recording ≳ 0.8, different < 0.65)."""
    best = 0.0
    for off in range(-80, 81):               # up to ~10 s of extra intro/silence on either copy
        pairs = [(a[i], b[i + off]) for i in range(max(0, -off), min(len(a), len(b) - off))]
        if len(pairs) < 20:
            continue
        diff = sum(bin(x ^ y).count("1") for x, y in pairs)
        best = max(best, 1 - diff / (32 * len(pairs)))
    return best


def same_audio_sync(a: str, b: str) -> bool | None:
    """True/False when both files could be fingerprinted, None when we can't tell."""
    fa, fb = _raw_fp(a), _raw_fp(b)
    if not fa or not fb:
        return None
    return similarity(fa, fb) >= 0.75


async def same_audio(a: str, b: str) -> bool | None:
    return await asyncio.to_thread(same_audio_sync, a, b)
