import asyncio
import shutil

from fastapi import APIRouter, Depends, HTTPException

from .. import __version__, auth, config, db
from ..services import lidarr, musicbrainz as mb, notify, slskd, spotify, ytdlp

router = APIRouter(prefix="/api/system", tags=["system"])


async def _check(name, coro):
    try:
        res = await asyncio.wait_for(coro, 10)
        return name, {"ok": True, "version": (res or {}).get("version") if isinstance(res, dict) else None}
    except Exception as e:  # noqa: BLE001
        return name, {"ok": False, "error": str(e)[:200]}


@router.get("/status")
async def status(user=Depends(auth.current_user)):
    checks = [_check("musicbrainz", mb.ping())]
    if lidarr.enabled():
        checks.append(_check("lidarr", lidarr.status()))
    if slskd.enabled():
        checks.append(_check("slskd", slskd.status()))
    res = dict(await asyncio.gather(*checks))
    res["ytdlp"] = {"ok": ytdlp.enabled(), "version": ytdlp.version(), "enabled": ytdlp.enabled()}
    res["lidarr"] = res.get("lidarr", {"ok": False, "enabled": False})
    res["slskd"] = res.get("slskd", {"ok": False, "enabled": False})
    res["spotify"] = {"ok": spotify.enabled(), "enabled": spotify.enabled()}
    res["ffmpeg"] = {"ok": bool(shutil.which("ffmpeg"))}
    try:
        du = shutil.disk_usage(config.get("library_path"))
        disk = {"total": du.total, "free": du.free}
    except OSError:
        disk = None
    return {"version": __version__, "services": res, "disk": disk, "app_name": config.get("app_name")}


@router.post("/test/{service}")
async def test(service: str, user=Depends(auth.admin_user)):
    try:
        if service == "lidarr":
            st = await lidarr.status()
            return {"ok": True, "message": f"Lidarr {st.get('version')}", "options": await lidarr.options()}
        if service == "slskd":
            st = await slskd.status()
            server = (st or {}).get("server", {})
            return {"ok": True, "message": f"slskd {(st or {}).get('version', {}).get('full', '')} – server {server.get('state', 'unknown')}"}
        if service == "musicbrainz":
            await mb.ping()
            return {"ok": True, "message": "MusicBrainz reachable"}
        if service == "notify":
            await notify.send("🔔 Test notification", f"{config.get('app_name')} notifications are working")
            return {"ok": True, "message": "Sent"}
        if service == "mediaservers":
            done = await notify.refresh_media_servers()
            return {"ok": bool(done), "message": ("Refreshed " + ", ".join(done)) if done else "No media servers configured"}
        if service == "spotify":
            await spotify._auth()
            return {"ok": True, "message": "Spotify credentials valid"}
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "message": str(e)[:300]}
    raise HTTPException(404, "Unknown service")


@router.get("/lidarr/options")
async def lidarr_options(user=Depends(auth.admin_user)):
    try:
        return await lidarr.options()
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))


@router.get("/downloads")
async def downloads(user=Depends(auth.admin_user)):
    out = {"slskd": [], "lidarr": [], "errors": {}}
    if slskd.enabled():
        try:
            out["slskd"] = await slskd.all_downloads()
        except Exception as e:  # noqa: BLE001
            out["errors"]["slskd"] = str(e)[:200]
    if lidarr.enabled():
        try:
            out["lidarr"] = await lidarr.queue()
        except Exception as e:  # noqa: BLE001
            out["errors"]["lidarr"] = str(e)[:200]
    return out


@router.post("/downloads/slskd/clear")
async def clear_slskd(user=Depends(auth.admin_user)):
    await slskd.clear_completed()
    return {"ok": True}


@router.get("/api-key")
def api_key(user=Depends(auth.admin_user)):
    return {"api_key": config.get("api_key")}


@router.get("/activity")
def activity(user=Depends(auth.admin_user)):
    return db.query("SELECT * FROM activity ORDER BY id DESC LIMIT 300")
