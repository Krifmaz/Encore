import asyncio
import json
import time

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import RedirectResponse
from pydantic import BaseModel

from .. import auth, config, db
from ..services import connections, playlists, sources, sync

router = APIRouter(prefix="/api", tags=["playlists"])


class ImportBody(BaseModel):
    url: str | None = None
    text: str | None = None
    name: str | None = None
    follow: bool = False
    into: int | None = None          # merge into an existing playlist
    request_missing: bool = False


class PlaylistPatch(BaseModel):
    name: str | None = None
    description: str | None = None
    follow: bool | None = None
    sync_targets: list[str] | None = None


class MergeBody(BaseModel):
    ids: list[int]
    name: str
    delete_sources: bool = False


class AddTracks(BaseModel):
    track_ids: list[int]


def _own(pid: int, user: dict) -> dict:
    pl = db.one("SELECT * FROM playlists WHERE id=?", (pid,))
    if not pl or pl["user_id"] != user["id"]:
        raise HTTPException(404, "Playlist not found")
    return pl


# ---- playlists ---------------------------------------------------------------------

@router.get("/playlists")
def list_playlists(user=Depends(auth.current_user)):
    playlists.get_liked(user["id"])
    rows = db.query("SELECT * FROM playlists WHERE user_id=? ORDER BY kind='liked' DESC, updated_at DESC", (user["id"],))
    return [playlists.summary(p) for p in rows]


@router.post("/playlists/preview")
async def preview(body: ImportBody, user=Depends(auth.current_user)):
    try:
        data = sources.parse_text(body.text) if body.text else await sources.fetch(body.url or "", user["id"])
    except sources.SourceError as e:
        raise HTTPException(400, str(e))
    from ..services.matcher import LibraryIndex
    rows = [dict(t) for t in data["tracks"]]
    matched = playlists.match_rows(rows, LibraryIndex.load())
    _, dupes = playlists._dedupe(rows)
    return {"name": data["name"], "source": data["source"], "cover": data.get("cover"), "description": data.get("description"),
            "count": len(rows), "in_library": matched, "duplicates": dupes, "tracks": rows[:300]}


@router.post("/playlists/import")
async def import_playlist(body: ImportBody, user=Depends(auth.current_user)):
    try:
        data = sources.parse_text(body.text) if body.text else await sources.fetch(body.url or "", user["id"])
    except sources.SourceError as e:
        raise HTTPException(400, str(e))
    if body.into:
        pl = _own(body.into, user)
        added, dupes = playlists.add_tracks(pl["id"], data["tracks"], data["source"])
        res = {"id": pl["id"], "added": added, "duplicates_skipped": dupes}
    else:
        res = playlists.create(user["id"], body.name or data["name"] or "Imported playlist", data["tracks"], data["source"],
                               body.url, data.get("description") or "", data.get("cover"), body.follow and bool(body.url))
    if body.request_missing:
        res["requested"] = playlists.request_missing(user, res["id"])
    db.log("info", f"{user['username']} imported '{data['name']}' from {data['source']} ({len(data['tracks'])} tracks, {res['duplicates_skipped']} duplicates skipped)")
    return res


@router.post("/playlists")
def create_playlist(body: PlaylistPatch, user=Depends(auth.current_user)):
    return playlists.create(user["id"], body.name or "New playlist", [], "manual", description=body.description or "")


@router.get("/playlists/{pid}")
def get_playlist(pid: int, user=Depends(auth.current_user)):
    pl = playlists.summary(_own(pid, user))
    rows = db.query("""SELECT i.*, t.album_id, t.format, t.bitrate, t.verify_status, r.status AS request_status
                       FROM playlist_items i LEFT JOIN tracks t ON t.id=i.track_id LEFT JOIN requests r ON r.id=i.request_id
                       WHERE i.playlist_id=? ORDER BY i.position, i.id""", (pid,))
    pl["items"] = rows
    pl["remote_ids"] = json.loads(pl.get("remote_ids") or "{}")
    return pl


@router.patch("/playlists/{pid}")
def patch_playlist(pid: int, body: PlaylistPatch, user=Depends(auth.current_user)):
    pl = _own(pid, user)
    fields = {}
    if body.name is not None and pl["kind"] != "liked":
        fields["name"] = body.name.strip() or pl["name"]
    if body.description is not None:
        fields["description"] = body.description
    if body.follow is not None:
        fields["follow"] = int(body.follow and bool(pl["source_url"]))
    if body.sync_targets is not None:
        allowed = {"jellyfin", "plex", "navidrome", "listenbrainz", "ytmusic"}
        src = pl["source"]
        fields["sync_targets"] = json.dumps([t for t in body.sync_targets if t in allowed and t != src])
    if fields:
        fields["updated_at"] = time.time()
        db.execute(f"UPDATE playlists SET {', '.join(k + '=?' for k in fields)} WHERE id=?", (*fields.values(), pid))
    return playlists.summary(db.one("SELECT * FROM playlists WHERE id=?", (pid,)))


@router.delete("/playlists/{pid}")
def delete_playlist(pid: int, user=Depends(auth.current_user)):
    pl = _own(pid, user)
    if pl["kind"] == "liked":
        raise HTTPException(400, "Liked Songs can't be deleted")
    db.execute("DELETE FROM playlist_items WHERE playlist_id=?", (pid,))
    db.execute("DELETE FROM playlists WHERE id=?", (pid,))
    return {"ok": True}


@router.post("/playlists/{pid}/dedupe")
def dedupe(pid: int, user=Depends(auth.current_user)):
    _own(pid, user)
    playlists.rematch(pid, only_unmatched=False)
    return {"removed": playlists.dedupe(pid)}


@router.post("/playlists/{pid}/rematch")
def rematch(pid: int, user=Depends(auth.current_user)):
    _own(pid, user)
    return {"matched": playlists.rematch(pid, only_unmatched=False)}


@router.post("/playlists/{pid}/request-missing")
def request_missing(pid: int, user=Depends(auth.current_user)):
    _own(pid, user)
    from .requests_routes import _quota
    _quota(user)
    n = playlists.request_missing(user, pid)
    return {"requested": n}


@router.post("/playlists/{pid}/refresh")
async def refresh(pid: int, user=Depends(auth.current_user)):
    pl = _own(pid, user)
    if not pl["source_url"]:
        raise HTTPException(400, "This playlist has no source to refresh from")
    try:
        added, dupes = await playlists.refresh_followed(pl)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))
    return {"added": added, "duplicates_skipped": dupes}


@router.post("/playlists/{pid}/tracks")
def add_tracks(pid: int, body: AddTracks, user=Depends(auth.current_user)):
    _own(pid, user)
    added, dupes = playlists.add_library_tracks(pid, body.track_ids)
    return {"added": added, "duplicates_skipped": dupes}


@router.delete("/playlists/{pid}/items/{item_id}")
def remove_item(pid: int, item_id: int, user=Depends(auth.current_user)):
    _own(pid, user)
    db.execute("DELETE FROM playlist_items WHERE id=? AND playlist_id=?", (item_id, pid))
    playlists.renumber(pid)
    return {"ok": True}


@router.post("/playlists/{pid}/move")
def move_item(pid: int, body: dict, user=Depends(auth.current_user)):
    _own(pid, user)
    rows = playlists.items(pid)
    ids = [r["id"] for r in rows]
    iid, to = int(body["item_id"]), int(body["to"])
    if iid not in ids:
        raise HTTPException(404, "Item not found")
    ids.remove(iid)
    ids.insert(max(0, min(to, len(ids))), iid)
    for pos, i in enumerate(ids):
        db.execute("UPDATE playlist_items SET position=? WHERE id=?", (pos, i))
    return {"ok": True}


@router.post("/playlists/merge")
def merge(body: MergeBody, user=Depends(auth.current_user)):
    if len(body.ids) < 2:
        raise HTTPException(400, "Pick at least two playlists")
    res = playlists.merge(user["id"], body.ids, body.name.strip() or "Merged playlist")
    if body.delete_sources:
        for pid in body.ids:
            pl = db.one("SELECT kind FROM playlists WHERE id=? AND user_id=?", (pid, user["id"]))
            if pl and pl["kind"] != "liked":
                db.execute("DELETE FROM playlist_items WHERE playlist_id=?", (pid,))
                db.execute("DELETE FROM playlists WHERE id=?", (pid,))
    return res


@router.post("/playlists/{pid}/sync")
async def sync_now(pid: int, user=Depends(auth.current_user)):
    _own(pid, user)
    log: list = []
    stats = await sync.sync_user(user["id"], log)
    return {**stats, "log": log}


# ---- likes -------------------------------------------------------------------------

@router.get("/likes")
def liked_ids(user=Depends(auth.current_user)):
    pl = playlists.get_liked(user["id"])
    return [r["track_id"] for r in db.query("SELECT track_id FROM playlist_items WHERE playlist_id=? AND track_id IS NOT NULL", (pl["id"],))]


@router.post("/likes/{track_id}")
def like(track_id: int, user=Depends(auth.current_user)):
    pl = playlists.get_liked(user["id"])
    added, _ = playlists.add_library_tracks(pl["id"], [track_id])
    return {"liked": True, "added": added}


@router.delete("/likes/{track_id}")
def unlike(track_id: int, user=Depends(auth.current_user)):
    pl = playlists.get_liked(user["id"])
    db.execute("DELETE FROM playlist_items WHERE playlist_id=? AND track_id=?", (pl["id"], track_id))
    playlists.renumber(pl["id"])
    return {"liked": False}


# ---- connections --------------------------------------------------------------------

class ConnectBody(BaseModel):
    username: str | None = None
    password: str | None = None
    token: str | None = None
    profile: str | None = None
    headers: str | None = None


class ConnPatch(BaseModel):
    sync_likes: bool | None = None
    sync_playlists: bool | None = None


@router.get("/connections")
def list_connections(user=Depends(auth.current_user)):
    have = {c["service"]: connections.public(c) for c in connections.list_for(user["id"])}
    avail = {
        "jellyfin": bool(config.get("jellyfin_url")), "plex": bool(config.get("plex_url")),
        "navidrome": bool(config.get("navidrome_url")), "spotify": bool(config.get("spotify_client_id")),
        "tidal": bool(config.get("tidal_client_id")), "deezer": True, "listenbrainz": True, "ytmusic": True,
        "lastfm": bool(config.get("lastfm_api_key")),
    }
    return {"connections": have, "available": avail, "services": {k: {"label": v[0], "kind": v[1]} for k, v in connections.SERVICES.items()},
            "sync": sync.state, "base_url": config.get("base_url")}


@router.post("/connections/{service}")
async def connect(service: str, body: ConnectBody, user=Depends(auth.current_user)):
    try:
        if service == "jellyfin":
            return await connections.connect_jellyfin(user["id"], body.username or "", body.password or "")
        if service == "navidrome":
            return await connections.connect_navidrome(user["id"], body.username or "", body.password or "")
        if service == "listenbrainz":
            return await connections.connect_listenbrainz(user["id"], (body.token or "").strip())
        if service == "lastfm":
            return await connections.connect_lastfm(user["id"], (body.username or "").strip())
        if service == "deezer":
            return await connections.connect_deezer(user["id"], body.profile or "")
        if service == "plex":
            return await connections.plex_pin_start()
        if service == "ytmusic":
            from ..services import ytmusic
            if body.headers:
                return await ytmusic.connect_browser(user["id"], body.headers)
            return await ytmusic.oauth_start()
        if service in ("spotify", "tidal"):
            return {"url": connections.oauth_start(user["id"], service)}
    except (connections.ConnectionError_, RuntimeError) as e:
        raise HTTPException(400, str(e))
    raise HTTPException(404, "Unknown service")


@router.post("/connections/plex/check")
async def plex_check(body: dict, user=Depends(auth.current_user)):
    try:
        return await connections.plex_pin_check(user["id"], int(body.get("pin_id", 0)))
    except connections.ConnectionError_ as e:
        raise HTTPException(400, str(e))


@router.post("/connections/ytmusic/check")
async def ytmusic_check(body: dict, user=Depends(auth.current_user)):
    from ..services import ytmusic
    try:
        return await ytmusic.oauth_check(user["id"], body.get("device_code", ""))
    except RuntimeError as e:
        raise HTTPException(400, str(e))


@router.get("/connections/{service}/callback")
async def oauth_callback(service: str, code: str = "", state: str = "", error: str = "", user=Depends(auth.current_user)):
    if error:
        return RedirectResponse(f"/#/connections?error={error}")
    try:
        await connections.oauth_finish(service, code, state, user["id"])
    except connections.ConnectionError_ as e:
        return RedirectResponse(f"/#/connections?error={str(e)[:200]}")
    return RedirectResponse(f"/#/connections?connected={service}")


@router.patch("/connections/{service}")
def patch_connection(service: str, body: ConnPatch, user=Depends(auth.current_user)):
    c = connections.get(user["id"], service)
    if not c:
        raise HTTPException(404, "Not connected")
    if body.sync_likes is not None:
        db.execute("UPDATE connections SET sync_likes=? WHERE id=?", (int(body.sync_likes), c["id"]))
    if body.sync_playlists is not None:
        db.execute("UPDATE connections SET sync_playlists=? WHERE id=?", (int(body.sync_playlists), c["id"]))
    return {"ok": True}


@router.delete("/connections/{service}")
def disconnect(service: str, user=Depends(auth.current_user)):
    c = connections.get(user["id"], service)
    if c:
        db.execute("DELETE FROM sync_state WHERE connection_id=?", (c["id"],))
        db.execute("DELETE FROM connections WHERE id=?", (c["id"],))
    return {"ok": True}


@router.get("/connections/{service}/playlists")
async def remote_playlists(service: str, user=Depends(auth.current_user)):
    c = connections.get(user["id"], service)
    if not c:
        raise HTTPException(404, "Not connected")
    try:
        return await sync.adapter(c).playlists()
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e)[:300])


@router.post("/connections/{service}/playlists/{rid}/import")
async def import_remote(service: str, rid: str, body: dict, user=Depends(auth.current_user)):
    c = connections.get(user["id"], service)
    if not c:
        raise HTTPException(404, "Not connected")
    name = body.get("name") or "Imported playlist"
    if body.get("url"):
        return await import_playlist(ImportBody(url=body["url"], name=name, follow=bool(body.get("follow"))), user)
    res = playlists.create(user["id"], name, [], service, f"{service}:{rid}", follow=bool(body.get("follow")))
    pl = db.one("SELECT * FROM playlists WHERE id=?", (res["id"],))
    added, dupes = await playlists.refresh_followed(pl)
    return {"id": res["id"], "added": added, "duplicates_skipped": dupes}


@router.post("/sync")
async def sync_me(user=Depends(auth.current_user)):
    log: list = []
    stats = await sync.sync_user(user["id"], log)
    return {**stats, "log": log}


@router.post("/sync/all")
async def sync_everyone(user=Depends(auth.admin_user)):
    asyncio.create_task(sync.sync_all())
    return {"ok": True}
