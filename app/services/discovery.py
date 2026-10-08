"""Discovery: ListenBrainz recommendations ("For you"), similar artists/tracks and genre browsing.

Sources: ListenBrainz (personal, MusicBrainz ids → exact matching), Last.fm (largest similarity data,
needs the free API key), Deezer (related artists, no key) as the fallback.
"""
import asyncio
import json
import re
import time

from .. import config, db
from . import connections, musicbrainz as mb
from .http import client

LB = "https://api.listenbrainz.org/1"
LB_LABS = "https://labs.api.listenbrainz.org"
LB_ALGO = "session_based_days_7500_session_300_contribution_3_threshold_10_limit_100_filter_True_skip_30"  # what listenbrainz.org's artist pages use
LASTFM = "https://ws.audioscrobbler.com/2.0/"
JSPF_PL = "https://musicbrainz.org/doc/jspf#playlist"
JSPF_TR = "https://musicbrainz.org/doc/jspf#track"

_cache: dict[str, tuple[float, object]] = {}


async def _cached(key: str, ttl: int, fn):
    hit = _cache.get(key)
    if hit and hit[0] > time.time():
        return hit[1]
    val = await fn()
    _cache[key] = (time.time() + ttl, val)
    if len(_cache) > 2000:
        for k in sorted(_cache, key=lambda k: _cache[k][0])[:500]:
            _cache.pop(k, None)
    return val


def _norm(s: str) -> str:
    return re.sub(r"[^\w]+", "", (s or "").lower())


def _library_artists() -> set[str]:
    return {r["n"] for r in db.query("SELECT DISTINCT lower(albumartist) n FROM tracks")}


# ---- ListenBrainz "For you" ----------------------------------------------------------------

def lb_user(user_id: int) -> str | None:
    c = connections.get(user_id, "listenbrainz")
    return (c or {}).get("data", {}).get("user") if c else None


def _kind(pl: dict) -> str:
    meta = (((pl.get("extension") or {}).get(JSPF_PL) or {}).get("additional_metadata") or {}).get("algorithm_metadata") or {}
    patch = meta.get("source_patch") or ""
    if patch:
        return patch
    t = (pl.get("title") or "").lower()
    return "weekly-exploration" if "exploration" in t else "daily-jams" if "daily" in t else "weekly-jams" if "weekly jams" in t else "other"


def _track(t: dict) -> dict:
    ids = t.get("identifier") or []
    ids = ids if isinstance(ids, list) else [ids]
    rec = next((i.rstrip("/").rsplit("/", 1)[-1] for i in ids if "musicbrainz.org/recording/" in i), None)
    ext = (t.get("extension") or {}).get(JSPF_TR) or {}
    add = ext.get("additional_metadata") or {}
    rel = (ext.get("release_identifier") or "").rstrip("/").rsplit("/", 1)[-1] or None
    cover = None
    if add.get("caa_release_mbid") and add.get("caa_id"):
        cover = f"https://coverartarchive.org/release/{add['caa_release_mbid']}/{add['caa_id']}-250.jpg"
    elif rel:
        cover = f"https://coverartarchive.org/release/{rel}/front-250"
    artist_ids = [a.rstrip("/").rsplit("/", 1)[-1] for a in ext.get("artist_identifiers") or []]
    return {"title": t.get("title") or "", "artist": t.get("creator") or "", "album": t.get("album") or "",
            "mbid": rec, "artist_mbid": artist_ids[0] if artist_ids else None, "release_mbid": rel,
            "length": (t.get("duration") or 0) / 1000 or None, "cover": cover}


async def lb_playlists(username: str) -> list[dict]:
    async def go():
        r = await client().get(f"{LB}/user/{username}/playlists/createdfor", params={"count": 25})
        if r.status_code == 404:
            raise ValueError(f"ListenBrainz user {username} not found")
        r.raise_for_status()
        out, seen = [], set()
        for item in r.json().get("playlists", []):
            pl = item.get("playlist") or item
            kind = _kind(pl)
            if kind in seen:          # newest of each kind only
                continue
            seen.add(kind)
            out.append({"id": (pl.get("identifier") or "").rstrip("/").rsplit("/", 1)[-1], "title": pl.get("title") or "",
                        "kind": kind, "date": pl.get("date") or "", "annotation": re.sub(r"<[^>]+>", "", pl.get("annotation") or "")[:400]})
        return out
    return await _cached(f"lbpl:{username}", 3600, go)


async def lb_playlist_tracks(pid: str) -> list[dict]:
    async def go():
        r = await client().get(f"{LB}/playlist/{pid}")
        r.raise_for_status()
        return [_track(t) for t in (r.json().get("playlist") or {}).get("track", [])]
    return await _cached(f"lbtr:{pid}", 6 * 3600, go)


def annotate_tracks(tracks: list[dict]) -> list[dict]:
    out = []
    for t in tracks:
        lt = db.one("""SELECT id, title, artist, album, album_id, format, duration FROM tracks WHERE mb_trackid=? LIMIT 1""", (t["mbid"],)) if t.get("mbid") else None
        lt = lt or (db.one("""SELECT id, title, artist, album, album_id, format, duration FROM tracks
                               WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)) LIMIT 1""",
                            (t["title"], t["artist"], t["artist"])) if t.get("title") else None)
        req = None
        if not lt:
            row = db.one("""SELECT status FROM requests WHERE kind='track' AND ((mbid IS NOT NULL AND mbid=?) OR (lower(artist)=lower(?) AND lower(title)=lower(?)))
                            AND status NOT IN ('failed','denied','cancelled') ORDER BY id DESC""", (t.get("mbid"), t["artist"], t["title"]))
            req = row["status"] if row else None
        out.append({**t, "library": lt, "request_status": req})
    return out


async def for_you(user: dict) -> dict:
    name = lb_user(user["id"])
    if not name:
        return {"connected": False, "playlists": []}
    pls = await lb_playlists(name)
    order = {"weekly-exploration": 0, "daily-jams": 1, "weekly-jams": 2}
    pls = sorted(pls, key=lambda p: order.get(p["kind"], 9))
    results = await asyncio.gather(*[lb_playlist_tracks(p["id"]) for p in pls], return_exceptions=True)
    out = []
    for p, tr in zip(pls, results):
        if isinstance(tr, Exception):
            continue
        out.append({**p, "tracks": annotate_tracks(tr)})
    return {"connected": True, "user": name, "playlists": out}


async def auto_discover() -> int:
    """Request the top N songs of each user's newest Weekly Exploration (once per playlist).
    Only for users whose downloads run automatically (admins, or auto-download switched on)."""
    n = int(config.get("discover_auto_count") or 0)
    if n <= 0:
        return 0
    from fastapi import HTTPException

    from ..routers.requests_routes import NewRequest, _create
    total = 0
    for c in db.query("SELECT * FROM connections WHERE service='listenbrainz'"):
        user = db.one("SELECT * FROM users WHERE id=? AND disabled=0", (c["user_id"],))
        if not user or not (user["role"] == "admin" or user.get("auto_download")):
            continue
        try:
            pls = [p for p in await lb_playlists(json.loads(c["data"])["user"]) if p["kind"] == "weekly-exploration"]
            if not pls or db.one("SELECT 1 FROM sync_state WHERE connection_id=? AND scope=?", (c["id"], f"discover:{pls[0]['id']}")):
                continue
            picked = 0
            for t in annotate_tracks(await lb_playlist_tracks(pls[0]["id"])):
                if picked >= n:
                    break
                if t["library"] or t["request_status"] or not (t["mbid"] or t["title"]):
                    continue
                try:
                    _create(user, NewRequest(kind="track", mbid=t["mbid"], artist=t["artist"], title=t["title"], album=t["album"] or None,
                                             cover_url=t["cover"], length=t["length"]), notify_admin=False)
                    picked += 1
                except HTTPException:
                    continue
            db.execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (?,?,?)", (c["id"], f"discover:{pls[0]['id']}", str(time.time())))
            if picked:
                db.log("info", f"Auto-discover: requested {picked} song(s) from {user['username']}'s {pls[0]['title']}")
            total += picked
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Auto-discover failed for {c['account']}: {e}")
    return total


# ---- similar artists -------------------------------------------------------------------------

async def _lastfm(method: str, **params) -> dict:
    key = config.get("lastfm_api_key")
    if not key:
        raise ValueError("no Last.fm key")
    r = await client().get(LASTFM, params={"method": method, "api_key": key, "format": "json", **params})
    d = r.json()
    if "error" in d:
        raise ValueError(d.get("message") or f"Last.fm error {d['error']}")
    return d


async def _deezer_picture(name: str) -> str | None:
    async def go():
        try:
            r = await client().get("https://api.deezer.com/search/artist", params={"q": name, "limit": 5})
            hits = [a for a in r.json().get("data", []) if _norm(a.get("name")) == _norm(name)]
            pic = max(hits, key=lambda a: a.get("nb_fan", 0)).get("picture_medium") if hits else None
            return None if not pic or "/artist//" in pic else pic
        except Exception:  # noqa: BLE001
            return None
    return await _cached(f"dzpic:{_norm(name)}", 7 * 86400, go)


async def _with_pictures(items: list[dict]) -> list[dict]:
    sem = asyncio.Semaphore(5)

    async def one(a):
        if not a.get("image"):
            async with sem:
                a["image"] = await _deezer_picture(a["name"])
        return a
    return list(await asyncio.gather(*[one(a) for a in items]))


async def similar_artists(mbid: str, name: str, limit: int = 18) -> dict:
    async def lastfm():
        p = {"mbid": mbid} if mbid else {"artist": name}
        try:
            d = await _lastfm("artist.getsimilar", limit=limit, autocorrect=1, **p)
        except ValueError:
            if not mbid:
                raise
            d = await _lastfm("artist.getsimilar", artist=name, limit=limit, autocorrect=1)
        return [{"name": a["name"], "mbid": a.get("mbid") or None, "score": float(a.get("match") or 0)}
                for a in (d.get("similarartists") or {}).get("artist", [])]

    async def listenbrainz():
        r = await client().get(f"{LB_LABS}/similar-artists/json", params={"algorithm": LB_ALGO, "artist_mbids": mbid})
        r.raise_for_status()
        rows, stack = [], [r.json()]
        while stack:                       # datasethoster output: possibly nested lists of rows
            x = stack.pop()
            if isinstance(x, list):
                stack.extend(reversed(x))
            elif isinstance(x, dict) and x.get("artist_mbid") and x.get("name"):
                rows.append(x)
        rows = [x for x in rows if x["artist_mbid"] != mbid]
        return [{"name": x["name"], "mbid": x["artist_mbid"], "score": x.get("score") or 0} for x in rows[:limit]]

    async def deezer():
        r = await client().get("https://api.deezer.com/search/artist", params={"q": name, "limit": 5})
        hits = [a for a in r.json().get("data", []) if _norm(a.get("name")) == _norm(name)]
        if not hits:
            return []
        aid = max(hits, key=lambda a: a.get("nb_fan", 0))["id"]
        rel = (await client().get(f"https://api.deezer.com/artist/{aid}/related", params={"limit": limit})).json().get("data", [])
        return [{"name": a["name"], "mbid": None, "image": a.get("picture_medium"), "score": a.get("nb_fan", 0)} for a in rel]

    async def go():
        for src, fn in (("Last.fm", lastfm), ("ListenBrainz", listenbrainz), ("Deezer", deezer)):
            if src == "Last.fm" and not config.get("lastfm_api_key"):
                continue
            if src == "ListenBrainz" and not mbid:
                continue
            try:
                items = await fn()
            except Exception:  # noqa: BLE001
                continue
            if items:
                return {"source": src, "artists": await _with_pictures(items[:limit])}
        return {"source": None, "artists": []}

    res = await _cached(f"sim:{mbid or _norm(name)}", 86400, go)
    have = _library_artists()
    return {**res, "artists": [{**a, "in_library": a["name"].lower() in have} for a in res["artists"]]}


async def similar_tracks(artist: str, title: str, mbid: str | None = None, limit: int = 25) -> dict:
    """"More like this" for a song: Last.fm track.getSimilar, else a Deezer artist radio."""
    async def go():
        if config.get("lastfm_api_key"):
            try:
                p = {"mbid": mbid} if mbid else {"artist": artist, "track": title}
                try:
                    d = await _lastfm("track.getsimilar", limit=limit, autocorrect=1, **p)
                except ValueError:
                    d = await _lastfm("track.getsimilar", artist=artist, track=title, limit=limit, autocorrect=1)
                items = [{"title": t["name"], "artist": (t.get("artist") or {}).get("name", ""), "mbid": t.get("mbid") or None,
                          "length": (t.get("duration") or 0) or None, "album": "", "cover": None}
                         for t in (d.get("similartracks") or {}).get("track", [])]
                if items:
                    return {"source": "Last.fm", "tracks": items}
            except Exception:  # noqa: BLE001
                pass
        r = await client().get("https://api.deezer.com/search/artist", params={"q": artist, "limit": 5})
        hits = [a for a in r.json().get("data", []) if _norm(a.get("name")) == _norm(artist)]
        if not hits:
            return {"source": None, "tracks": []}
        radio = (await client().get(f"https://api.deezer.com/artist/{hits[0]['id']}/radio", params={"limit": limit})).json().get("data", [])
        return {"source": "Deezer radio", "tracks": [{"title": t.get("title_short") or t.get("title"), "artist": (t.get("artist") or {}).get("name", ""),
                                                      "mbid": None, "length": t.get("duration"), "album": (t.get("album") or {}).get("title", ""),
                                                      "cover": (t.get("album") or {}).get("cover_medium"), "isrc": t.get("isrc")} for t in radio]}
    res = await _cached(f"simtr:{mbid or _norm(artist + title)}", 86400, go)
    return {**res, "tracks": annotate_tracks(res["tracks"])}


# ---- genres / tags -----------------------------------------------------------------------------

async def tag(tag: str, page: int = 1) -> dict:
    async def go():
        if config.get("lastfm_api_key"):
            try:
                d = await _lastfm("tag.gettopartists", tag=tag, limit=48, page=page)
                arts = [{"name": a["name"], "mbid": a.get("mbid") or None} for a in (d.get("topartists") or {}).get("artist", [])]
                info = {}
                try:
                    info = (await _lastfm("tag.getinfo", tag=tag)).get("tag") or {}
                except Exception:  # noqa: BLE001
                    pass
                summary = re.sub(r"<a [^>]*>.*?</a>\.?", "", ((info.get("wiki") or {}).get("summary") or "")).strip()
                if arts:
                    return {"source": "Last.fm", "summary": summary, "artists": await _with_pictures(arts)}
            except Exception:  # noqa: BLE001
                pass
        d = await mb._get("artist", {"query": f'tag:"{tag}"', "limit": 48, "offset": (page - 1) * 48})
        arts = [{"name": a["name"], "mbid": a["id"]} for a in d.get("artists", [])]
        return {"source": "MusicBrainz", "summary": "", "artists": await _with_pictures(arts)}
    res = await _cached(f"tag:{tag.lower()}:{page}", 86400, go)
    have = _library_artists()
    return {**res, "tag": tag, "artists": [{**a, "in_library": a["name"].lower() in have} for a in res["artists"]]}


async def top_tags() -> list[str]:
    async def go():
        if config.get("lastfm_api_key"):
            try:
                d = await _lastfm("tag.getTopTags")
                return [t["name"] for t in (d.get("toptags") or {}).get("tag", [])][:40]
            except Exception:  # noqa: BLE001
                pass
        return ["rock", "pop", "electronic", "hip hop", "jazz", "metal", "indie", "folk", "punk", "soul", "r&b", "ambient",
                "house", "techno", "classical", "country", "blues", "reggae", "experimental", "shoegaze", "drum and bass", "dubstep"]
    return await _cached("toptags", 7 * 86400, go)


def library_tags(limit: int = 30) -> list[str]:
    rows = db.query("SELECT lower(genre) g, COUNT(*) n FROM tracks WHERE genre IS NOT NULL AND genre!='' GROUP BY lower(genre) ORDER BY n DESC LIMIT ?", (limit,))
    return [r["g"] for r in rows]

