"""Admin → Diagnostics: run every connection/path/tool check and show the raw results."""
import asyncio
import os
import shutil
import subprocess
import time

from fastapi import APIRouter, Depends

from .. import __version__, auth, config
from ..services import acoustid, streamrip
from ..services.http import client, explain

router = APIRouter(prefix="/api/system", tags=["system"], dependencies=[Depends(auth.admin_user)])

SECRET_KEYS = [k for k, (_, _, secret) in config.DEFAULTS.items() if secret]


def _redact(text: str) -> str:
    for k in SECRET_KEYS:
        v = str(config.get(k) or "")
        if len(v) >= 4:
            text = text.replace(v, "•••")
    return text


async def _http(name: str, method: str, url: str, ok_status=(200,), **kw) -> dict:
    out = {"name": name, "kind": "service", "request": f"{method} {_redact(url)}"}
    t0 = time.monotonic()
    try:
        r = await client().request(method, url, timeout=10, **kw)
    except Exception as e:  # noqa: BLE001
        return {**out, "ok": False, "ms": int((time.monotonic() - t0) * 1000), "detail": explain(name, url, exc=e)}
    body = _redact(r.text[:600])
    ok = r.status_code in ok_status
    return {**out, "ok": ok, "status": r.status_code, "ms": int((time.monotonic() - t0) * 1000), "body": body,
            "detail": "OK" if ok else explain(name, url, r.status_code, r.text)}


def _path_check(label: str, path: str, need_write: bool = True) -> dict:
    out = {"name": label, "kind": "path", "request": path}
    if not path:
        return {**out, "ok": False, "detail": "Not set"}
    if not os.path.isdir(path):
        return {**out, "ok": False, "detail": f"{path} doesn't exist inside the container – check the volume mapping in your compose file"}
    st = os.stat(path)
    me = f"uid {os.getuid()}/gid {os.getgid()}"
    owner = f"owned by uid {st.st_uid}/gid {st.st_gid}"
    if need_write:
        probe = os.path.join(path, f".encore-write-test-{os.getpid()}")
        try:
            with open(probe, "w") as fh:
                fh.write("x")
            os.remove(probe)
        except OSError as e:
            return {**out, "ok": False, "detail": f"Not writable by Encore ({me}); folder is {owner}. Fix with: chown -R {os.getuid()}:{os.getgid()} <host folder> or set PUID/PGID ({e.strerror})"}
    try:
        n = len(os.listdir(path))
    except OSError as e:
        return {**out, "ok": False, "detail": f"Can't list folder: {e.strerror}"}
    return {**out, "ok": True, "detail": f"OK – {n} entries, {owner}, Encore runs as {me}"}


def _tool(name: str, cmd: list[str]) -> dict:
    out = {"name": name, "kind": "tool", "request": " ".join(cmd)}
    if not shutil.which(cmd[0]):
        return {**out, "ok": False, "detail": "Not installed"}
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        line = (r.stdout or r.stderr).strip().splitlines()
        return {**out, "ok": r.returncode == 0, "detail": line[0][:160] if line else f"exit {r.returncode}"}
    except Exception as e:  # noqa: BLE001
        return {**out, "ok": False, "detail": str(e)}


@router.get("/diagnostics")
async def diagnostics():
    g = config.get
    checks: list = []
    tasks = [_http("MusicBrainz", "GET", g("musicbrainz_url").rstrip("/") + "/ws/2/artist?query=test&limit=1&fmt=json")]
    if g("lidarr_url"):
        tasks.append(_http("Lidarr", "GET", g("lidarr_url").rstrip("/") + "/api/v1/system/status", headers={"X-Api-Key": g("lidarr_api_key")}))
    if g("slskd_url"):
        tasks.append(_http("slskd", "GET", g("slskd_url").rstrip("/") + "/api/v0/application", headers={"X-API-Key": g("slskd_api_key")}))
    if g("jellyfin_url"):
        tasks.append(_http("Jellyfin", "GET", g("jellyfin_url").rstrip("/") + "/System/Info/Public"))
    if g("plex_url"):
        tasks.append(_http("Plex", "GET", g("plex_url").rstrip("/") + "/identity", headers={"Accept": "application/json", "X-Plex-Token": g("plex_token")}))
    if g("navidrome_url"):
        tasks.append(_http("Navidrome", "GET", g("navidrome_url").rstrip("/") + "/rest/ping?v=1.16.1&c=encore&f=json"))
    if g("acoustid_api_key"):
        tasks.append(_http("AcoustID", "GET", f"https://api.acoustid.org/v2/lookup?client={g('acoustid_api_key')}&trackid=9ff43b6a-4f16-427c-93c2-92307ca505e0"))
    tasks.append(_http("Cover Art Archive", "GET", "https://coverartarchive.org/", ok_status=(200, 301, 302, 307, 308)))
    tasks.append(_http("YouTube Music", "GET", "https://music.youtube.com/", ok_status=(200, 301, 302, 303)))
    checks += await asyncio.gather(*tasks)
    checks += [
        _path_check("Music library", g("library_path")),
        _path_check("Staging folder", g("staging_path")),
        _path_check("slskd downloads (as seen by Encore)", g("slskd_download_path")) if g("slskd_enabled") or g("slskd_url") else None,
        _path_check("Config folder", os.environ.get("ENCORE_CONFIG_DIR", "/config")),
    ]
    checks += await asyncio.to_thread(lambda: [
        _tool("ffmpeg", ["ffmpeg", "-version"]),
        _tool("fpcalc (audio fingerprints)", ["fpcalc", "-version"]) if shutil.which("fpcalc") else
        {"name": "fpcalc (audio fingerprints)", "kind": "tool", "request": "fpcalc", "ok": acoustid.can_fingerprint(),
         "detail": "Not installed – using ffmpeg's chromaprint instead" if acoustid.can_fingerprint() else "Not installed and ffmpeg has no chromaprint"},
        _tool("yt-dlp", ["yt-dlp", "--version"]) if shutil.which("yt-dlp") else {"name": "yt-dlp", "kind": "tool", "request": "python yt_dlp",
                                                                                "ok": True, "detail": __import__("yt_dlp").version.__version__},
        _tool("streamrip", [streamrip.binary() or "streamrip", "--version"]),
        _tool("curl", ["curl", "--version"]),
    ])
    settings = []
    for k, (env, _, secret) in config.DEFAULTS.items():
        if k in ("api_key", "plex_client_id"):
            continue
        src = config.source(k)
        if src == "default":
            continue
        val = config.get(k)
        settings.append({"key": k, "source": src, "env": env if src == "env" else None,
                         "value": config.mask(val) if secret else val})
    return {"version": __version__, "uid": os.getuid(), "checks": [c for c in checks if c], "settings": settings,
            "time": time.time()}
