"""Fill MusicBrainz gaps: merge Deezer, Apple Music, YouTube Music, Bandcamp and Discogs discographies onto MusicBrainz.

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


def _dur(v) -> float | None:
    """'3:45' / 'PT3M45S' / 'P00H03M45S' → seconds."""
    if not v:
        return None
    if isinstance(v, (int, float)):
        return float(v)
    m = re.fullmatch(r"P?T?(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?", str(v).replace("P00H", "PT0H"))
    if m and any(m.groups()):
        h, mi, se = (int(x or 0) for x in m.groups())
        return h * 3600 + mi * 60 + se
    parts = [p for p in str(v).split(":") if p.isdigit()]
    return float(sum(int(p) * 60 ** i for i, p in enumerate(reversed(parts)))) if parts else None


# Discogs: deep catalogue (vinyl, cassettes, bootlegs, old releases). Free token; ~60 requests/minute.
DISCOGS = "https://api.discogs.com"


def _discogs_headers() -> dict:
    from .. import __version__
    from .. import config
    h = {"User-Agent": f"Encore/{__version__} +https://github.com/Krifmaz/Encore"}
    if config.get("discogs_token"):
        h["Authorization"] = f"Discogs token={config.get('discogs_token')}"
    return h


async def discogs_artist(name: str, urls: list[str]) -> int | None:
    from .. import config
    for u in urls:
        m = re.search(r"discogs\.com/(?:\w+/)?artist/(\d+)", u)
        if m:
            return int(m.group(1))
    if not config.get("discogs_token"):
        return None                     # search needs a token; MusicBrainz links work without
    r = await client().get(f"{DISCOGS}/database/search", params={"q": name, "type": "artist", "per_page": 10}, headers=_discogs_headers())
    hits = [a for a in r.json().get("results", []) if norm(re.sub(r"\s*\(\d+\)$", "", a.get("title", ""))) == norm(name)]
    return hits[0]["id"] if hits else None


def _discogs_type(fmt: str) -> str:
    f = (fmt or "").lower()
    return "Single" if "single" in f else "EP" if "ep" in re.split(r"[,\s]+", f) else "Compilation" if "comp" in f else "Album"


async def discogs_albums(aid: int) -> list[dict]:
    out, page = [], 1
    while page <= 3:
        r = await client().get(f"{DISCOGS}/artists/{aid}/releases", params={"sort": "year", "per_page": 100, "page": page}, headers=_discogs_headers())
        d = r.json()
        for x in d.get("releases", []):
            if x.get("role", "Main") != "Main" or x.get("type") not in ("master", "release"):
                continue
            kind = "masters" if x["type"] == "master" else "releases"
            out.append({"title": x.get("title", ""), "type": _discogs_type(x.get("format", "")), "date": str(x.get("year") or ""),
                        "cover": x.get("thumb") or None, "tracks": None, "explicit": False,
                        "sources": [{"name": "Discogs", "id": f"{kind[0]}{x['id']}", "url": f"https://www.discogs.com/{kind[:-1]}/{x['id']}"}]})
        if page >= (d.get("pagination") or {}).get("pages", 1):
            break
        page += 1
    return out


# Bandcamp: indie/underground. No API – the artist's /music page and each album's JSON-LD.
async def bandcamp_artist(name: str, urls: list[str]) -> str | None:
    for u in urls:
        m = re.match(r"(https?://[\w-]+\.bandcamp\.com)", u)
        if m:
            return m.group(1)
    r = await client().get("https://bandcamp.com/search", params={"q": name, "item_type": "b"})
    for m in re.finditer(r'<div class="heading">\s*<a href="(https://[\w-]+\.bandcamp\.com)[^"]*">\s*([^<]+?)\s*</a>', r.text):
        if norm(m.group(2)) == norm(name):
            return m.group(1)
    return None


def _ld(html: str) -> dict:
    for m in re.finditer(r'<script type="application/ld\+json"[^>]*>(.*?)</script>', html, re.S):
        try:
            import json
            d = json.loads(m.group(1))
            if d.get("@type") in ("MusicAlbum", "MusicRecording"):
                return d
        except ValueError:
            continue
    return {}


def _bc_album(base: str, url: str, ld: dict, fallback_title: str = "", cover: str | None = None) -> dict:
    from datetime import datetime
    date = ""
    for fmt in ("%d %b %Y %H:%M:%S %Z", "%Y-%m-%d"):
        try:
            date = datetime.strptime(ld.get("datePublished", ""), fmt).strftime("%Y-%m-%d")
            break
        except ValueError:
            continue
    n = ld.get("numTracks") or len(((ld.get("track") or {}).get("itemListElement")) or []) or (1 if ld.get("@type") == "MusicRecording" else None)
    title = ld.get("name") or fallback_title
    img = ld.get("image")
    return {"title": title, "type": "Single" if "/track/" in url or n == 1 else "EP" if n and n <= 6 else "Album", "date": date,
            "cover": (img[0] if isinstance(img, list) else img) or cover, "tracks": n, "explicit": False,
            "sources": [{"name": "Bandcamp", "id": url, "url": url}]}


async def bandcamp_albums(base: str) -> list[dict]:
    r = await client().get(f"{base}/music", follow_redirects=True)
    html = r.text
    if "/album/" in str(r.url) or "/track/" in str(r.url):          # single-release artists land on the release itself
        return [_bc_album(base, str(r.url), _ld(html))]
    items = []
    for m in re.finditer(r'<a href="((?:https://[\w-]+\.bandcamp\.com)?/(?:album|track)/[^"?]+)"[^>]*>(.*?)</a>', html, re.S):
        href, inner = m.group(1), m.group(2)
        t = re.search(r'<p class="title">\s*(.*?)\s*(?:<br|</p>)', inner, re.S)
        img = re.search(r'<img[^>]+(?:data-original|src)="([^"]+)"', inner)
        url = href if href.startswith("http") else base + href
        if t and url not in [i[0] for i in items]:
            items.append((url, re.sub(r"\s+", " ", t.group(1)).strip(), img.group(1) if img else None))
    sem = asyncio.Semaphore(4)

    async def one(url, title, img):
        async with sem:
            try:
                return _bc_album(base, url, _ld((await client().get(url)).text), title, img)
            except Exception:  # noqa: BLE001
                return _bc_album(base, url, {}, title, img)
    return list(await asyncio.gather(*[one(*i) for i in items[:30]]))


# YouTube Music: mixtapes, singles and things nobody else lists (ytmusicapi, no key).
async def ytm_artist(name: str, urls: list[str]) -> str | None:
    from . import ytmusic
    for u in urls:
        m = re.search(r"(?:music\.)?youtube\.com/channel/(UC[\w-]+)", u)
        if m:
            return m.group(1)
    res = await ytmusic.run(ytmusic.anon().search, name, "artists", None, 10)
    hits = [a for a in res if norm(a.get("artist") or a.get("title") or "") == norm(name) and a.get("browseId")]
    return hits[0]["browseId"] if hits else None


async def ytm_albums(channel: str) -> list[dict]:
    from . import ytmusic
    a = await ytmusic.run(ytmusic.anon().get_artist, channel)
    out = []
    for key, kind in (("albums", "Album"), ("singles", "Single")):
        sec = a.get(key) or {}
        items = sec.get("results") or []
        if sec.get("browseId") and sec.get("params"):
            try:
                items = await ytmusic.run(ytmusic.anon().get_artist_albums, sec["browseId"], sec["params"])
            except Exception:  # noqa: BLE001
                pass
        for x in items:
            if not x.get("browseId"):
                continue
            t = (x.get("type") or kind)
            out.append({"title": x.get("title", ""), "type": "EP" if t == "EP" else "Single" if t == "Single" else "Album",
                        "date": str(x.get("year") or ""), "cover": ((x.get("thumbnails") or [{}])[-1]).get("url"), "tracks": None,
                        "explicit": bool(x.get("isExplicit")),
                        "sources": [{"name": "YouTube Music", "id": x["browseId"], "url": f"https://music.youtube.com/browse/{x['browseId']}"}]})
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
    if ext.get("ytm"):
        from . import ytmusic
        a = await ytmusic.run(ytmusic.anon().get_album, ext["ytm"])
        return [{"title": t.get("title", ""), "artist": ", ".join(x.get("name", "") for x in t.get("artists") or []), "length": t.get("duration_seconds"),
                 "isrc": None, "track": t.get("trackNumber") or i, "disc": 1, "recording_id": None} for i, t in enumerate(a.get("tracks", []), 1)]
    if ext.get("bandcamp"):
        ld = _ld((await client().get(ext["bandcamp"])).text)
        items = ((ld.get("track") or {}).get("itemListElement")) or ([{"position": 1, "item": ld}] if ld.get("@type") == "MusicRecording" else [])
        return [{"title": (x.get("item") or {}).get("name", ""), "artist": "", "length": _dur((x.get("item") or {}).get("duration")),
                 "isrc": None, "track": x.get("position") or i, "disc": 1, "recording_id": None} for i, x in enumerate(items, 1)]
    if ext.get("discogs"):
        kind = "masters" if str(ext["discogs"]).startswith("m") else "releases"
        d = (await client().get(f"{DISCOGS}/{kind}/{str(ext['discogs'])[1:]}", headers=_discogs_headers())).json()
        tl = [t for t in d.get("tracklist", []) if t.get("type_", "track") == "track"]
        return [{"title": t.get("title", ""), "artist": ", ".join(a.get("name", "") for a in t.get("artists") or []), "length": _dur(t.get("duration")),
                 "isrc": None, "track": i, "disc": 1, "recording_id": None} for i, t in enumerate(tl, 1)]
    return []


EXT_KEYS = {"Deezer": "deezer", "Apple Music": "apple", "YouTube Music": "ytm", "Bandcamp": "bandcamp", "Discogs": "discogs"}


def ext_for(m: dict) -> dict:
    """What a request needs to fetch this release: {deezer: id, apple: id, ytm: browseId, bandcamp: url, discogs: m123/r456, upc}."""
    out = {EXT_KEYS[s["name"]]: s["id"] for s in m["sources"] if s["name"] in EXT_KEYS}
    if m.get("upc"):
        out["upc"] = m["upc"]
    return out


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

    name = artist["name"]
    services = [("deezer.com", deezer_artist, deezer_albums), ("apple.com", apple_artist, apple_albums),
                ("youtube.com", ytm_artist, ytm_albums), ("bandcamp.com", bandcamp_artist, bandcamp_albums),
                ("discogs.com", discogs_artist, discogs_albums)]
    ids = await asyncio.gather(*[safe(find(name, urls), None) for _, find, _ in services])

    async def albums(i):
        return await safe(services[i][2](ids[i]), []) if ids[i] else []
    *lists, barcodes = await asyncio.gather(*[albums(i) for i in range(len(services))], safe(mb.artist_barcodes(artist["id"]), {}))
    # a name-only artist match must share at least one release with MusicBrainz, or it's someone else
    for (linked, _, _), found, lst in zip(services, ids, lists):
        if found and lst and rgs and not any(linked in u for u in urls) and not any(_fuzzy(x, rgs) for x in lst):
            lst.clear()
    rg_ids = {rg["id"] for rg in rgs}
    rgs_by_id = {rg["id"]: rg for rg in rgs}
    budget = {"deep": MAX_DEEP, "isrc": MAX_ISRC}
    missing: list[dict] = []
    for ext in [x for lst in lists for x in lst]:
        if _fuzzy(ext, rgs):
            continue
        # same release from the other service → one entry with both sources
        twin = next((m for m in missing if similar(m["title"], ext["title"]) >= 0.92
                     and (m["type"] == ext["type"] or "Discogs" in [s["name"] for s in m["sources"] + ext["sources"]])), None)
        if twin:
            if not any(s["name"] == ext["sources"][0]["name"] for s in twin["sources"]):
                twin["sources"] += ext["sources"]
            twin["cover"] = twin["cover"] or ext["cover"]
            twin["tracks"] = twin["tracks"] or ext["tracks"]
            twin["date"] = twin["date"] if len(twin["date"]) >= len(ext["date"]) else ext["date"]
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
        m["harmony"] = harmony_url([s["url"] for s in m["sources"] if s["name"] != "Discogs"] or [m["sources"][0]["url"]], m.get("upc"))
        m["ext"] = ext_for(m)
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
