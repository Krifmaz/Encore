"""Lidarr v1 API client."""
from .. import config
from .http import client


class LidarrError(Exception):
    pass


def enabled() -> bool:
    return bool(config.get("lidarr_enabled") and config.get("lidarr_url") and config.get("lidarr_api_key"))


async def _req(method: str, path: str, **kw):
    url = config.get("lidarr_url").rstrip("/") + "/api/v1/" + path.lstrip("/")
    r = await client().request(method, url, headers={"X-Api-Key": config.get("lidarr_api_key")}, **kw)
    if r.status_code >= 400:
        raise LidarrError(f"Lidarr {method} {path} → {r.status_code}: {r.text[:300]}")
    return r.json() if r.content else None


async def status() -> dict:
    return await _req("GET", "system/status")


async def options() -> dict:
    return {
        "quality_profiles": [{"id": p["id"], "name": p["name"]} for p in await _req("GET", "qualityprofile")],
        "metadata_profiles": [{"id": p["id"], "name": p["name"]} for p in await _req("GET", "metadataprofile")],
        "root_folders": [{"path": f["path"], "free": f.get("freeSpace")} for f in await _req("GET", "rootfolder")],
    }


async def _defaults() -> tuple[str, int, int]:
    root = config.get("lidarr_root_folder")
    qp = config.get("lidarr_quality_profile")
    mp = config.get("lidarr_metadata_profile")
    if not (root and qp and mp):
        opts = await options()
        root = root or (opts["root_folders"][0]["path"] if opts["root_folders"] else "")
        qp = qp or (opts["quality_profiles"][0]["id"] if opts["quality_profiles"] else 1)
        mp = mp or (opts["metadata_profiles"][0]["id"] if opts["metadata_profiles"] else 1)
    if not root:
        raise LidarrError("No root folder configured in Lidarr")
    return root, qp, mp


async def find_artist(mbid: str) -> dict | None:
    for a in await _req("GET", "artist"):
        if a.get("foreignArtistId") == mbid:
            return a
    return None


async def ensure_artist(mbid: str, monitor_all: bool = False) -> dict:
    existing = await find_artist(mbid)
    if existing:
        if monitor_all and not existing.get("monitored"):
            existing["monitored"] = True
            await _req("PUT", f"artist/{existing['id']}", json=existing)
        return existing
    found = await _req("GET", "artist/lookup", params={"term": f"lidarr:{mbid}"})
    if not found:
        raise LidarrError("Artist not found in Lidarr metadata")
    root, qp, mp = await _defaults()
    body = found[0]
    body.update({
        "qualityProfileId": qp,
        "metadataProfileId": mp,
        "rootFolderPath": root,
        "monitored": True,
        "monitorNewItems": "all" if monitor_all else "none",
        "addOptions": {"monitor": "all" if monitor_all else "none", "searchForMissingAlbums": monitor_all},
    })
    return await _req("POST", "artist", json=body)


async def album_by_rg(rgid: str) -> dict | None:
    res = await _req("GET", "album", params={"foreignAlbumId": rgid})
    return res[0] if res else None


async def monitor_and_search(album_id: int) -> None:
    await _req("PUT", "album/monitor", json={"albumIds": [album_id], "monitored": True})
    await _req("POST", "command", json={"name": "AlbumSearch", "albumIds": [album_id]})


async def refresh_artist(artist_id: int) -> None:
    await _req("POST", "command", json={"name": "RefreshArtist", "artistId": artist_id})


async def album(album_id: int) -> dict:
    return await _req("GET", f"album/{album_id}")


async def queue() -> list[dict]:
    data = await _req("GET", "queue", params={"page": 1, "pageSize": 100, "includeArtist": "true", "includeAlbum": "true"})
    out = []
    for q in data.get("records", []):
        size = q.get("size") or 0
        left = q.get("sizeleft") or 0
        out.append({
            "id": q["id"],
            "artist": (q.get("artist") or {}).get("artistName", ""),
            "album": (q.get("album") or {}).get("title", ""),
            "status": q.get("status", ""),
            "state": q.get("trackedDownloadState", ""),
            "protocol": q.get("protocol", ""),
            "client": q.get("downloadClient", ""),
            "progress": round((1 - left / size) * 100, 1) if size else 0,
            "timeleft": q.get("timeleft", ""),
            "messages": [m for s in q.get("statusMessages", []) for m in s.get("messages", [])],
        })
    return out


async def artist_rg_ids() -> set[str]:
    """Release-group ids of albums Lidarr already has files for."""
    out = set()
    for a in await _req("GET", "album", params={"includeAllArtistAlbums": "true"}) or []:
        if (a.get("statistics") or {}).get("trackFileCount", 0) > 0:
            out.add(a.get("foreignAlbumId"))
    return out
