from fastapi import APIRouter, Depends, HTTPException

from .. import auth, config, db
from ..services import artistinfo, catalog, discovery, library, musicbrainz as mb, ytdlp

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
async def artist(mbid: str, user=Depends(auth.current_user)):
    try:
        a = await mb.artist(mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    annotate_albums(a["release_groups"])
    a["request_status"] = _req_status([mbid]).get(mbid)
    f = db.one("SELECT id, types FROM follows WHERE user_id=? AND mbid=?", (user["id"], mbid))
    a["follow"] = {"id": f["id"], "types": __import__("json").loads(f["types"])} if f else None
    a["library_albums"] = db.query(
        """SELECT album_id, album, albumartist, year, COUNT(*) tracks, MAX(mb_releasegroupid) rgid FROM tracks
           WHERE mb_albumartistid=? OR lower(albumartist)=lower(?) GROUP BY album_id ORDER BY year DESC""",
        (mbid, a["name"]),
    )
    a["library_tracks"] = db.one(
        "SELECT COUNT(*) n FROM tracks WHERE mb_albumartistid=? OR mb_artistid=? OR lower(albumartist)=lower(?) OR lower(artist)=lower(?)",
        (mbid, mbid, a["name"], a["name"]))["n"]
    return a


@router.get("/artist/{mbid}/extras")
async def artist_extras(mbid: str, user=Depends(auth.current_user)):
    """Photo, Wikipedia bio and popular tracks (best-effort, loaded after the page)."""
    try:
        a = await mb.artist(mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    return await artistinfo.extras(mbid, a["name"], a["links"])


@router.get("/artist/{mbid}/library-tracks")
def artist_library_tracks(mbid: str, name: str = "", user=Depends(auth.current_user)):
    """Every track by this artist in the library, for Play / Shuffle."""
    return db.query("""SELECT id, title, artist, album, album_id, format, duration FROM tracks
                       WHERE mb_albumartistid=? OR mb_artistid=? OR lower(albumartist)=lower(?) OR lower(artist)=lower(?)
                       ORDER BY year DESC, lower(album), disc, track LIMIT 5000""", (mbid, mbid, name, name))


@router.get("/artist/{mbid}/similar")
async def artist_similar(mbid: str, user=Depends(auth.current_user)):
    """Similar artists: Last.fm → ListenBrainz → Deezer."""
    try:
        a = await mb.artist(mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    return await discovery.similar_artists(mbid, a["name"])


@router.post("/catalog/tracks")
async def catalog_tracks(ext: dict, user=Depends(auth.current_user)):
    """Track list of a release that isn't on MusicBrainz (Deezer / Apple / YouTube Music / Bandcamp / Discogs)."""
    try:
        return await catalog.tracklist(ext)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"Could not load the track list: {e}")


@router.get("/artist/{mbid}/catalog")
async def artist_catalog(mbid: str, user=Depends(auth.current_user)):
    """Releases on Deezer / Apple Music that MusicBrainz doesn't list for this artist."""
    if not config.get("discover_catalog_merge"):
        return {"enabled": False, "releases": []}
    try:
        a = await mb.artist(mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    return {"enabled": True, "releases": catalog.annotate([dict(x) for x in await catalog.merged(a)])}


@router.get("/for-you")
async def for_you(user=Depends(auth.current_user)):
    try:
        return await discovery.for_you(user)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"ListenBrainz error: {e}")


@router.get("/similar-tracks")
async def similar_tracks(artist: str, title: str, mbid: str | None = None, user=Depends(auth.current_user)):
    try:
        return await discovery.similar_tracks(artist, title, mbid)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))


@router.get("/tags")
async def tags(user=Depends(auth.current_user)):
    return {"top": await discovery.top_tags(), "library": discovery.library_tags()}


@router.get("/tag/{name}")
async def tag(name: str, page: int = 1, user=Depends(auth.current_user)):
    try:
        return await discovery.tag(name, max(1, page))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))


@router.get("/preview")
async def preview(artist: str, title: str, isrc: str | None = None, length: float | None = None, user=Depends(auth.current_user)):
    """30-second preview clip for a song you don't have yet."""
    p = await discovery.preview_track(artist, title, isrc, length)
    if not p:
        raise HTTPException(404, "No preview available for this song")
    return p


@router.get("/preview-album")
async def preview_album(artist: str = "", title: str = "", key: str = "", user=Depends(auth.current_user)):
    dz = int(key[3:]) if key.startswith("dz:") and key[3:].isdigit() else None
    if not dz and not (artist and title):
        raise HTTPException(400, "artist + title or key needed")
    return await discovery.preview_album(artist, title, dz)


@router.get("/new-releases")
async def new_releases(limit: int = 24, user=Depends(auth.current_user)):
    try:
        return {"releases": await discovery.new_releases(user["id"], limit=min(limit, 60))}
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, str(e))


@router.get("/artist-lookup")
async def artist_lookup(name: str, user=Depends(auth.current_user)):
    """Find the MusicBrainz artist for a library artist name (tag first, then an exact-name search)."""
    row = db.one("SELECT MAX(mb_albumartistid) id FROM tracks WHERE lower(albumartist)=lower(?)", (name,))
    if row and row["id"]:
        return {"mbid": row["id"]}
    try:
        hits = await mb.search_artists(name, 5)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"MusicBrainz error: {e}")
    exact = [h for h in hits if h["name"].lower() == name.lower()]
    if not exact:
        raise HTTPException(404, "No MusicBrainz artist with that name")
    return {"mbid": max(exact, key=lambda h: h.get("score", 0))["id"]}


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
    try:
        info = await ytdlp.probe(url)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(400, f"Unsupported or unreachable URL: {str(e)[:200]}")
    return {"source": "ytdlp", **info}
