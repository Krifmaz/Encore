"""Fill MusicBrainz gaps: merge Deezer + Apple Music discographies onto the MusicBrainz one.

Matching, strongest first: barcode (UPC ↔ MusicBrainz release barcode) → ISRC of the first track
(→ MusicBrainz recording → release group) → normalized title + year + track count. Releases that
match nothing are shown as "Not in MusicBrainz" and can be requested from slskd/streamrip/YouTube
directly (Lidarr can't monitor them). Each links to Harmony so it can be added to MusicBrainz.
"""
import asyncio
import re
import time
from difflib import SequenceMatcher
from urllib.parse import urlencode

from . import library, musicbrainz as mb
from .http import client

_cache: dict[str, tuple[float, list]] = {}
TTL = 24 * 3600
MAX_DEEP = 25        # Deezer album lookups (UPC + ISRC) per artist
MAX_ISRC = 8         # MusicBrainz ISRC lookups per artist (MusicBrainz allows 1 request/second)


def norm(title: str) -> str:
    t = (title or "").lower()
    t = re.sub(r"\s+-\s+(single|ep)$", "", t)
    t = re.sub(r"[\(\[][^\)\]]*(deluxe|edition|remaster|version|expanded|anniversary|explicit|clean|bonus)[^\)\]]*[\)\]]", "", t)
    t = t.replace("&", "and")
    return re.sub(r"[^\w]+", "", t)


def similar(a: str, b: str) -> float:
    a, b = norm(a), norm(b)
    if not a or not b:
        return 0.0
    return 1.0 if a == b else SequenceMatcher(None, a, b).ratio()


def harmony_url(urls: list[str], upc: str | None = None) -> str:
    q = [("url", u) for u in urls]
    if upc:
        q.append(("gtin", upc))
    return "https://harmony.pulsewidth.org.uk/release?" + urlencode(q + [("category", "preferred")])


def _type(kind: str, title: str = "", n: int | None = None) -> str:
    k = (kind or "").lower()
    if k in ("ep",) or title.endswith(" - EP"):
        return "EP"
    if k in ("single",) or title.endswith(" - Single"):
        return "Single"
    if k in ("compile", "compilation"):
        return "Compilation"
    return "Album"


def _clean_title(t: str) -> str:
    return re.sub(r"\s+-\s+(Single|EP)$", "", t or "")


# ---- finding the artist on each service -------------------------------------------------

async def deezer_artist(name: str, urls: list[str]) -> int | None:
    for u in urls:
        m = re.search(r"deezer\.com/(?:\w+/)?artist/(\d+)", u)
        if m:
            return int(m.group(1))
    r = await client().get("https://api.deezer.com/search/artist", params={"q": name, "limit": 10})
    hits = [a for a in r.json().get("data", []) if norm(a.get("name")) == norm(name)]
    return max(hits, key=lambda a: a.get("nb_fan", 0))["id"] if hits else None


async def apple_artist(name: str, urls: list[str]) -> int | None:
    for u in urls:
        m = re.search(r"(?:music|itunes)\.apple\.com/.*?/artist/(?:[^/]+/)?(?:id)?(\d+)", u)
        if m:
            return int(m.group(1))
    r = await client().get("https://itunes.apple.com/search", params={"term": name, "entity": "musicArtist", "limit": 10})
    hits = [a for a in r.json().get("results", []) if norm(a.get("artistName")) == norm(name)]
    return hits[0]["artistId"] if hits else None


async def deezer_albums(aid: int) -> list[dict]:
    r = await client().get(f"https://api.deezer.com/artist/{aid}/albums", params={"limit": 300})
    out = []
    for a in r.json().get("data", []):
        out.append({"title": a.get("title", ""), "type": _type(a.get("record_type"), a.get("title", "")),
                    "date": a.get("release_date") or "", "cover": a.get("cover_xl") or a.get("cover_medium"),
                    "tracks": a.get("nb_tracks"), "explicit": a.get("explicit_lyrics"),
                    "sources": [{"name": "Deezer", "id": a["id"], "url": f"https://www.deezer.com/album/{a['id']}"}]})
    return out


async def apple_albums(aid: int) -> list[dict]:
    r = await client().get("https://itunes.apple.com/lookup", params={"id": aid, "entity": "album", "limit": 200})
    out = []
    for a in r.json().get("results", []):
        if a.get("wrapperType") != "collection":
            continue
        title = a.get("collectionName", "")
        out.append({"title": _clean_title(title), "type": _type("", title, a.get("trackCount")),
                    "date": (a.get("releaseDate") or "")[:10], "cover": (a.get("artworkUrl100") or "").replace("100x100bb", "600x600bb") or None,
                    "tracks": a.get("trackCount"), "explicit": a.get("collectionExplicitness") == "explicit",
                    "sources": [{"name": "Apple Music", "id": a["collectionId"], "url": a.get("collectionViewUrl") or f"https://music.apple.com/album/{a['collectionId']}"}]})
    return out


# ---- tracklists (used when requesting a release MusicBrainz doesn't have) ------------------

async def tracklist(ext: dict) -> list[dict]:
    if ext.get("deezer"):
        r = await client().get(f"https://api.deezer.com/album/{ext['deezer']}/tracks", params={"limit": 500})
        return [{"title": (t.get("title") or ""), "artist": (t.get("artist") or {}).get("name", ""), "length": t.get("duration"),
                 "isrc": t.get("isrc"), "track": t.get("track_position") or i, "disc": t.get("disk_number") or 1, "recording_id": None}
                for i, t in enumerate(r.json().get("data", []), 1)]
    if ext.get("apple"):
        r = await client().get("https://itunes.apple.com/lookup", params={"id": ext["apple"], "entity": "song", "limit": 300})
        return [{"title": t.get("trackName", ""), "artist": t.get("artistName", ""), "length": (t.get("trackTimeMillis") or 0) / 1000 or None,
                 "isrc": None, "track": t.get("trackNumber"), "disc": t.get("discNumber") or 1, "recording_id": None}
                for t in r.json().get("results", []) if t.get("wrapperType") == "track"]
    return []


# ---- merging -------------------------------------------------------------------------------

def _fuzzy(ext: dict, rgs: list[dict]) -> dict | None:
    y = (ext.get("date") or "")[:4]
    for rg in rgs:
        if "Single" in (ext["type"], rg.get("type")) and ext["type"] != rg.get("type"):
            continue   # a title-track single isn't the album
        s = similar(ext["title"], rg["title"])
        if s >= 0.92 or (s >= 0.8 and y and rg.get("year") == y):
            return rg
    return None


async def _deep_match(ext: dict, barcodes: dict[str, str], rg_ids: set[str], rgs_by_id: dict, budget: dict) -> str | None:
    dz = next((s for s in ext["sources"] if s["name"] == "Deezer"), None)
    if not dz or budget["deep"] <= 0:
        return None
    budget["deep"] -= 1
    d = (await client().get(f"https://api.deezer.com/album/{dz['id']}")).json()
    upc = (d.get("upc") or "").lstrip("0")
    ext["upc"] = d.get("upc")
    if upc and upc in barcodes:
        return barcodes[upc]
    isrc = next((t.get("isrc") for t in (d.get("tracks") or {}).get("data", []) if t.get("isrc")), None)
    if isrc is None and (d.get("tracks") or {}).get("data"):
        t = (await client().get(f"https://api.deezer.com/track/{d['tracks']['data'][0]['id']}")).json()
        isrc = t.get("isrc")
    if isrc and budget["isrc"] > 0:
        budget["isrc"] -= 1
        for rgid in await mb.release_groups_for_isrc(isrc) & rg_ids:
            if similar(ext["title"], rgs_by_id[rgid]["title"]) >= 0.6:   # same song on a single vs. album isn't the same release
                return rgid
    return None


async def merged(artist: dict) -> list[dict]:
    """Releases on Deezer/Apple that MusicBrainz doesn't list for this artist."""
    hit = _cache.get(artist["id"])
    if hit and hit[0] > time.time():
        return hit[1]
    rgs = artist["release_groups"]
    urls = artist.get("urls") or []

    async def safe(coro, default):
        try:
            return await coro
        except Exception:  # noqa: BLE001 – one service failing never breaks the page
            return default

    dz_id, ap_id = await asyncio.gather(safe(deezer_artist(artist["name"], urls), None), safe(apple_artist(artist["name"], urls), None))
    dz, ap, barcodes = await asyncio.gather(safe(deezer_albums(dz_id), []) if dz_id else asyncio.sleep(0, []),
                                            safe(apple_albums(ap_id), []) if ap_id else asyncio.sleep(0, []),
                                            safe(mb.artist_barcodes(artist["id"]), {}))
    # a name-only artist match must share at least one release with MusicBrainz, or it's someone else
    for found, lst, linked in ((dz_id, dz, "deezer.com"), (ap_id, ap, "apple.com")):
        if found and lst and rgs and not any(linked in u for u in urls) and not any(_fuzzy(x, rgs) for x in lst):
            lst.clear()
    rg_ids = {rg["id"] for rg in rgs}
    rgs_by_id = {rg["id"]: rg for rg in rgs}
    budget = {"deep": MAX_DEEP, "isrc": MAX_ISRC}
    missing: list[dict] = []
    for ext in dz + ap:
        if _fuzzy(ext, rgs):
            continue
        # same release from the other service → one entry with both sources
        twin = next((m for m in missing if similar(m["title"], ext["title"]) >= 0.92 and m["type"] == ext["type"]), None)
        if twin:
            twin["sources"] += ext["sources"]
            twin["cover"] = twin["cover"] or ext["cover"]
            continue
        try:
            if await _deep_match(ext, barcodes, rg_ids, rgs_by_id, budget):
                continue
        except Exception:  # noqa: BLE001
            pass
        missing.append(ext)
    for m in missing:
        m["id"] = "ext-" + "-".join(f"{s['name'][0].lower()}{s['id']}" for s in m["sources"])
        m["year"] = (m["date"] or "")[:4]
        m["artist"] = artist["name"]
        m["harmony"] = harmony_url([s["url"] for s in m["sources"]], m.get("upc"))
    missing.sort(key=lambda m: m["date"], reverse=True)
    _cache[artist["id"]] = (time.time() + TTL, missing)
    return missing


def annotate(items: list[dict]) -> list[dict]:
    from .. import db
    for m in items:
        m["in_library"] = library.has_album(m["artist"], m["title"])
        r = db.one("""SELECT status FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                      AND status NOT IN ('failed','denied','cancelled') ORDER BY id DESC""", (m["artist"], m["title"]))
        m["request_status"] = r["status"] if r else None
    return items
