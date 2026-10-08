"""MusicBrainz web service client with rate limiting and caching."""
import asyncio
import time

from .. import __version__, config
from .http import client, loop_local

_last = 0.0
_cache: dict[str, tuple[float, dict]] = {}
CACHE_TTL = 3600


def _ua() -> str:
    contact = config.get("contact_email") or "https://github.com/Krifmaz/Encore"
    return f"Encore/{__version__} ( {contact} )"


async def _get(path: str, params: dict | None = None, fresh: bool = False) -> dict:
    params = dict(params or {})
    params["fmt"] = "json"
    key = path + "?" + "&".join(f"{k}={v}" for k, v in sorted(params.items()))
    hit = _cache.get(key)
    if hit and hit[0] > time.time() and not fresh:
        return hit[1]
    global _last
    base = config.get("musicbrainz_url").rstrip("/")
    for attempt in range(4):
        async with loop_local("musicbrainz", asyncio.Lock):
            wait = (1.0 / max(config.get("musicbrainz_rate"), 0.1)) - (time.time() - _last)
            if wait > 0:
                await asyncio.sleep(wait)
            _last = time.time()
            r = await client().get(f"{base}/ws/2/{path}", params=params, headers={"User-Agent": _ua()})
        if r.status_code == 503:
            await asyncio.sleep(1.5 * (attempt + 1))
            continue
        r.raise_for_status()
        data = r.json()
        _cache[key] = (time.time() + CACHE_TTL, data)
        if len(_cache) > 3000:
            for k in list(_cache)[:500]:
                _cache.pop(k, None)
        return data
    raise RuntimeError("MusicBrainz is rate limiting requests, try again shortly")


def esc(s: str) -> str:
    out = []
    for ch in s or "":
        if ch in '+-&|!(){}[]^"~*?:\\/':
            out.append("\\" + ch)
        else:
            out.append(ch)
    return "".join(out)


def credit(ac: list | None) -> str:
    if not ac:
        return ""
    return "".join((c.get("name") or c.get("artist", {}).get("name", "")) + c.get("joinphrase", "") for c in ac)


def credit_id(ac: list | None) -> str | None:
    if ac:
        return ac[0].get("artist", {}).get("id")
    return None


def cover_rg(rgid: str, size: int = 250) -> str:
    return f"https://coverartarchive.org/release-group/{rgid}/front-{size}"


def cover_release(rid: str, size: int = 250) -> str:
    return f"https://coverartarchive.org/release/{rid}/front-{size}"


# ---- search -----------------------------------------------------------------

async def search_artists(q: str, limit: int = 20) -> list[dict]:
    data = await _get("artist", {"query": q, "limit": limit})
    return [
        {
            "id": a["id"],
            "name": a["name"],
            "disambiguation": a.get("disambiguation", ""),
            "type": a.get("type", ""),
            "country": a.get("country", ""),
            "score": a.get("score", 0),
            "begin": (a.get("life-span") or {}).get("begin", ""),
            "tags": [t["name"] for t in sorted(a.get("tags", []), key=lambda t: -t.get("count", 0))[:4]],
        }
        for a in data.get("artists", [])
    ]


def _rg(rg: dict) -> dict:
    return {
        "id": rg["id"],
        "title": rg.get("title", ""),
        "artist": credit(rg.get("artist-credit")),
        "artist_id": credit_id(rg.get("artist-credit")),
        "type": rg.get("primary-type") or "",
        "secondary": rg.get("secondary-types", []),
        "year": (rg.get("first-release-date") or "")[:4],
        "date": rg.get("first-release-date") or "",
        "score": rg.get("score", 0),
        "cover": cover_rg(rg["id"]),
    }


async def search_albums(q: str, limit: int = 25) -> list[dict]:
    data = await _get("release-group", {"query": q, "limit": limit})
    return [_rg(rg) for rg in data.get("release-groups", [])]


async def search_recordings(q: str, limit: int = 25, artist: str | None = None) -> list[dict]:
    query = q
    if artist:
        query = f'recording:"{esc(q)}" AND artist:"{esc(artist)}"'
    data = await _get("recording", {"query": query, "limit": limit})
    out = []
    for r in data.get("recordings", []):
        rel = (r.get("releases") or [{}])[0]
        rg = rel.get("release-group") or {}
        out.append({
            "id": r["id"],
            "title": r.get("title", ""),
            "artist": credit(r.get("artist-credit")),
            "artist_id": credit_id(r.get("artist-credit")),
            "length": (r.get("length") or 0) / 1000,
            "album": rel.get("title", ""),
            "release_id": rel.get("id"),
            "release_group_id": rg.get("id"),
            "year": (rel.get("date") or r.get("first-release-date") or "")[:4],
            "score": r.get("score", 0),
            "cover": cover_rg(rg["id"]) if rg.get("id") else None,
        })
    return out


async def search_releases(artist: str, album: str, limit: int = 15) -> list[dict]:
    q = f'release:"{esc(album)}"'
    if artist:
        q += f' AND artist:"{esc(artist)}"'
    data = await _get("release", {"query": q, "limit": limit})
    out = []
    for r in data.get("releases", []):
        out.append({
            "id": r["id"],
            "title": r.get("title", ""),
            "artist": credit(r.get("artist-credit")),
            "date": r.get("date", ""),
            "country": r.get("country", ""),
            "status": r.get("status", ""),
            "track_count": r.get("track-count", 0),
            "formats": ", ".join(sorted({m.get("format") or "?" for m in r.get("media", [])})),
            "release_group_id": (r.get("release-group") or {}).get("id"),
            "type": (r.get("release-group") or {}).get("primary-type", ""),
            "score": r.get("score", 0),
            "label": ", ".join(li.get("label", {}).get("name", "") for li in r.get("label-info", []) if li.get("label")),
            "cover": cover_release(r["id"]),
        })
    return out


# ---- lookups ----------------------------------------------------------------

async def artist(mbid: str, fresh: bool = False) -> dict:
    a = await _get(f"artist/{mbid}", {"inc": "tags+url-rels"}, fresh)
    rgs = []
    offset = 0
    while True:
        page = await _get("release-group", {"artist": mbid, "limit": 100, "offset": offset}, fresh)
        rgs += page.get("release-groups", [])
        offset += 100
        if offset >= page.get("release-group-count", 0) or offset >= 2000:
            break
    groups = [_rg({**rg, "artist-credit": [{"name": a["name"], "artist": {"id": mbid}}]}) for rg in rgs]
    groups.sort(key=lambda g: g["year"] or "9999", reverse=True)
    links, urls = {}, []
    for rel in a.get("relations", []):
        url = (rel.get("url") or {}).get("resource")
        if url:
            links[rel.get("type", "link")] = url
            urls.append(url)
    return {
        "id": a["id"],
        "name": a["name"],
        "disambiguation": a.get("disambiguation", ""),
        "type": a.get("type", ""),
        "country": a.get("country", ""),
        "begin": (a.get("life-span") or {}).get("begin", ""),
        "tags": [t["name"] for t in sorted(a.get("tags", []), key=lambda t: -t.get("count", 0))[:8]],
        "links": links,
        "urls": urls,
        "release_groups": groups,
    }


async def artist_barcodes(mbid: str) -> dict[str, str]:
    """barcode (UPC/EAN) → release-group id for every release by the artist."""
    out: dict[str, str] = {}
    offset = 0
    while offset < 400:
        page = await _get("release", {"artist": mbid, "inc": "release-groups", "limit": 100, "offset": offset})
        for rel in page.get("releases", []):
            bc = (rel.get("barcode") or "").lstrip("0")
            if bc and rel.get("release-group"):
                out[bc] = rel["release-group"]["id"]
        offset += 100
        if offset >= page.get("release-count", 0):
            break
    return out


async def release_groups_for_isrc(isrc: str) -> set[str]:
    out: set[str] = set()
    for rid in (await recordings_for_isrc(isrc))[:2]:
        try:
            d = await _get(f"recording/{rid}", {"inc": "releases+release-groups"})
        except Exception:  # noqa: BLE001
            continue
        out |= {(rel.get("release-group") or {}).get("id") for rel in d.get("releases", [])} - {None}
    return out


async def release_group(rgid: str) -> dict:
    rg = await _get(f"release-group/{rgid}", {"inc": "artist-credits+releases+tags"})
    return {
        **_rg(rg),
        "tags": [t["name"] for t in sorted(rg.get("tags", []), key=lambda t: -t.get("count", 0))[:5]],
        "releases": rg.get("releases", []),
    }


async def best_release_for_group(rgid: str, track_count: int | None = None) -> str | None:
    """Pick the most canonical release in a release group (optionally matching a track count)."""
    data = await _get("release", {"release-group": rgid, "inc": "media", "limit": 100})
    releases = data.get("releases", [])
    if not releases:
        return None

    def score(r):
        tc = sum(m.get("track-count", 0) for m in r.get("media", []))
        s = 0.0
        if track_count:
            s -= abs(tc - track_count) * 10
        if r.get("status") == "Official":
            s += 5
        fmts = {m.get("format") for m in r.get("media", [])}
        if "Digital Media" in fmts:
            s += 3
        if "CD" in fmts:
            s += 2
        if r.get("country") in ("XW", "US", "GB"):
            s += 1
        s -= (int((r.get("date") or "9999")[:4] or 9999) - 1900) / 1000.0
        return s

    return max(releases, key=score)["id"]


async def release(rid: str) -> dict:
    r = await _get(f"release/{rid}", {"inc": "recordings+artist-credits+release-groups+labels+genres+isrcs"})
    rg = r.get("release-group") or {}
    tracks = []
    media = r.get("media", [])
    for m in media:
        for t in m.get("tracks", []):
            rec = t.get("recording") or {}
            tracks.append({
                "disc": m.get("position", 1),
                "track": int(t.get("position") or t.get("number") or 0),
                "title": t.get("title") or rec.get("title", ""),
                "artist": credit(t.get("artist-credit") or rec.get("artist-credit")),
                "artist_id": credit_id(t.get("artist-credit") or rec.get("artist-credit")),
                "length": (t.get("length") or rec.get("length") or 0) / 1000,
                "recording_id": rec.get("id"),
                "track_id": t.get("id"),
                "isrc": (rec.get("isrcs") or [None])[0],
            })
    genres = [g["name"] for g in sorted(r.get("genres", []) + rg.get("genres", []), key=lambda g: -g.get("count", 0))]
    return {
        "id": r["id"],
        "title": r.get("title", ""),
        "artist": credit(r.get("artist-credit")),
        "artist_id": credit_id(r.get("artist-credit")),
        "date": r.get("date") or rg.get("first-release-date") or "",
        "year": (rg.get("first-release-date") or r.get("date") or "")[:4],
        "country": r.get("country", ""),
        "status": r.get("status", ""),
        "release_group_id": rg.get("id"),
        "type": rg.get("primary-type", ""),
        "label": ", ".join(li.get("label", {}).get("name", "") for li in r.get("label-info", []) if li.get("label")),
        "catalog": ", ".join(li.get("catalog-number") or "" for li in r.get("label-info", []) if li.get("catalog-number")),
        "barcode": r.get("barcode") or "",
        "disc_count": len(media),
        "genre": genres[0].title() if genres else "",
        "tracks": tracks,
        "cover": cover_release(r["id"], 500),
    }


async def recording(rid: str) -> dict:
    r = await _get(f"recording/{rid}", {"inc": "artist-credits+releases+release-groups+isrcs"})
    rels = r.get("releases", [])
    rel = rels[0] if rels else {}
    rg = rel.get("release-group") or {}
    return {
        "id": r["id"],
        "title": r.get("title", ""),
        "artist": credit(r.get("artist-credit")),
        "artist_id": credit_id(r.get("artist-credit")),
        "length": (r.get("length") or 0) / 1000,
        "album": rel.get("title", ""),
        "release_id": rel.get("id"),
        "release_group_id": rg.get("id"),
        "year": (r.get("first-release-date") or rel.get("date") or "")[:4],
        "isrc": (r.get("isrcs") or [None])[0],
    }


async def ping() -> bool:
    await _get("artist", {"query": "test", "limit": 1})
    return True


async def recordings_for_isrc(isrc: str) -> list[str]:
    try:
        d = await _get(f"isrc/{isrc}")
    except Exception:  # noqa: BLE001
        return []
    return [r["id"] for r in d.get("recordings", [])]


async def album_for_recording(rid: str, allow_ep: bool = True) -> dict | None:
    """The official studio album (or EP) a recording first appeared on, if any.

    Compilations, live albums, soundtracks, remix albums and DJ mixes are skipped, so asking for one song
    never pulls in "Now That's What I Call Music" or a greatest-hits collection.
    """
    d = await _get(f"recording/{rid}", {"inc": "releases+release-groups+artist-credits"})
    artist_ids = {c.get("artist", {}).get("id") for c in d.get("artist-credit", [])}
    types = ("Album", "EP") if allow_ep else ("Album",)
    best, best_key = None, None
    for rel in d.get("releases", []):
        rg = rel.get("release-group") or {}
        if rel.get("status") not in (None, "Official"):
            continue
        if rg.get("primary-type") not in types or rg.get("secondary-types"):
            continue
        # skip releases by other artists (e.g. "Various Artists" or a featured artist's album)
        rel_artists = {c.get("artist", {}).get("id") for c in rel.get("artist-credit", [])} or artist_ids
        if artist_ids and not (rel_artists & artist_ids):
            continue
        key = (rg.get("primary-type") != "Album", rg.get("first-release-date") or rel.get("date") or "9999")
        if best_key is None or key < best_key:
            best, best_key = rg, key
    if not best:
        return None
    return {"id": best["id"], "title": best.get("title"), "type": best.get("primary-type"),
            "artist": credit(d.get("artist-credit")), "year": (best.get("first-release-date") or "")[:4],
            "cover": cover_rg(best["id"])}


async def resolve_recording(artist: str, title: str, length: float | None = None, isrc: str | None = None) -> str | None:
    """Find the MusicBrainz recording for a song, version-aware (a remix won't resolve to the original)."""
    from . import matcher
    if isrc:
        ids = await recordings_for_isrc(isrc)
        if ids:
            return ids[0]
    want = matcher.describe(title, artist, length=length)
    for r in await search_recordings(title, 10, artist=artist):
        if matcher.compare(want, matcher.describe(r["title"], r["artist"], length=r["length"] or None)).match:
            return r["id"]
    return None
