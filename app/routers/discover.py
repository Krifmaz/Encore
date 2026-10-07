from fastapi import APIRouter, Depends, HTTPException

from .. import auth, db
from ..services import library, musicbrainz as mb, spotify, ytdlp

router = APIRouter(prefix="/api/discover", tags=["discover"], dependencies=[Depends(auth.current_user)])


def _req_status(mbids: list[str]) -> dict:
    if not mbids:
        return {}
    rows = db.query(
        f"SELECT mbid, status FROM requests WHERE mbid IN ({','.join('?' * len(mbids))}) AND status NOT IN ('denied','cancelled') ORDER BY id",
        mbids,
    )
    return {r["mbid"]: r["status"] for r in rows}


def annotate_albums(items: list[dict]) -> list[dict]:
    st = _req_status([i["id"] for i in items])
    for i in items:
        i["in_library"] = library.has_release_group(i["id"]) or library.has_album(i["artist"], i["title"])
        i["request_status"] = st.get(i["id"])
    return items


@router.get("/search")
async def search(q: str, type: str = "all"):
    q = q.strip()
    if len(q) < 2:
        return {"artists": [], "albums": [], "tracks": []}
    try:
        out = {"artists": [], "albums": [], "tracks": []}
        if type in ("all", "artist"):
            out["artists"] = await mb.search_artists(q, 12 if type == "all" else 30)
            names = {a["name"].lower() for a in out["artists"]}
            have = {r["n"] for r in db.query("SELECT DISTINCT lower(albumartist) n FROM tracks")} & names
            for a in out["artists"]:
                a["in_library"] = a["name"].lower() in have
        if type in ("all", "album"):
            out["albums"] = annotate_albums(await mb.search_albums(q, 18 if type == "all" else 40))
        if type in ("all", "track"):
            tracks = await mb.search_recordings(q, 15 if type == "all" else 40)
            st = _req_status([t["id"] for t in tracks])
            for t in tracks:
                t["in_library"] = library.has_track(t["id"], t["artist"], t["title"])
                t["request_status"] = st.get(t["id"])
            out["tracks"] = tracks
        return out
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")


@router.get("/artist/{mbid}")
async def artist(mbid: str):
    try:
        a = await mb.artist(mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    annotate_albums(a["release_groups"])
    a["request_status"] = _req_status([mbid]).get(mbid)
    a["library_albums"] = db.query(
        "SELECT album_id, album, year, COUNT(*) tracks FROM tracks WHERE mb_albumartistid=? OR lower(albumartist)=lower(?) GROUP BY album_id ORDER BY year DESC",
        (mbid, a["name"]),
    )
    return a


@router.get("/album/{rgid}")
async def album(rgid: str):
    try:
        rg = await mb.release_group(rgid)
        rid = await mb.best_release_for_group(rgid)
        rel = await mb.release(rid) if rid else None
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    rg.pop("releases", None)
    annotate_albums([rg])
    if rel:
        for t in rel["tracks"]:
            t["in_library"] = library.has_track(t["recording_id"], t["artist"], t["title"])
    rg["release"] = rel
    return rg


@router.get("/probe")
async def probe(url: str):
    if spotify.parse(url):
        if not spotify.enabled():
            raise HTTPException(400, "Spotify import needs a Client ID/Secret in Settings → Integrations")
        data = await spotify.tracks(url)
        return {"source": "spotify", "title": data["name"], "count": len(data["tracks"]), "tracks": data["tracks"][:500], "is_playlist": True}
    try:
        info = await ytdlp.probe(url)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(400, f"Unsupported or unreachable URL: {str(e)[:200]}")
    return {"source": "ytdlp", **info}
