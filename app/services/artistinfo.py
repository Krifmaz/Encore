"""Artist page extras: photo, short bio and popular tracks (Deezer + Wikipedia, no keys needed)."""
import re
import time
from urllib.parse import quote, unquote

from .. import db
from .http import client

_cache: dict[str, tuple[float, dict]] = {}
TTL = 24 * 3600


def _norm(s: str) -> str:
    return re.sub(r"[^\w]+", "", (s or "").lower())


def library_track(artist: str, title: str) -> dict | None:
    return db.one("""SELECT id, title, artist, album, album_id, format, duration FROM tracks
                     WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?))
                     ORDER BY CASE WHEN format IN ('flac','alac','wav','aiff') THEN 0 ELSE 1 END LIMIT 1""", (title, artist, artist))


async def _deezer(name: str) -> dict:
    out: dict = {}
    r = await client().get("https://api.deezer.com/search/artist", params={"q": name, "limit": 5})
    hits = [a for a in r.json().get("data", []) if _norm(a.get("name")) == _norm(name)]
    if not hits:
        return out
    a = max(hits, key=lambda a: a.get("nb_fan", 0))
    if "/artist//" not in (a.get("picture_xl") or "/artist//"):
        out["image"] = a["picture_xl"]
    out["fans"] = a.get("nb_fan")
    top = (await client().get(f"https://api.deezer.com/artist/{a['id']}/top", params={"limit": 10})).json().get("data", [])
    out["top"] = [{"title": t.get("title_short") or t.get("title"), "version": t.get("title_version") or "",
                   "album": (t.get("album") or {}).get("title", ""), "cover": (t.get("album") or {}).get("cover_medium"),
                   "duration": t.get("duration"), "isrc": t.get("isrc"), "rank": t.get("rank")} for t in top]
    return out


async def _wikipedia(links: dict) -> dict:
    title = None
    wp = links.get("wikipedia")
    if wp and "en.wikipedia.org/wiki/" in wp:
        title = unquote(wp.rsplit("/wiki/", 1)[1])
    elif links.get("wikidata"):
        qid = links["wikidata"].rstrip("/").rsplit("/", 1)[-1]
        d = (await client().get(f"https://www.wikidata.org/wiki/Special:EntityData/{qid}.json")).json()
        title = (((d.get("entities") or {}).get(qid) or {}).get("sitelinks") or {}).get("enwiki", {}).get("title")
    if not title:
        return {}
    s = (await client().get(f"https://en.wikipedia.org/api/rest_v1/page/summary/{quote(title.replace(' ', '_'), safe='')}")).json()
    out = {"bio": s.get("extract") or "", "bio_url": ((s.get("content_urls") or {}).get("desktop") or {}).get("page")}
    if (s.get("originalimage") or s.get("thumbnail") or {}).get("source"):
        out["wiki_image"] = (s.get("originalimage") or s.get("thumbnail"))["source"]
    return out


async def extras(mbid: str, name: str, links: dict) -> dict:
    hit = _cache.get(mbid)
    if hit and hit[0] > time.time():
        data = hit[1]
    else:
        data = {"image": None, "bio": "", "bio_url": None, "top": []}
        for fn in (lambda: _deezer(name), lambda: _wikipedia(links)):
            try:
                data.update(await fn())
            except Exception:  # noqa: BLE001 – extras are best-effort
                pass
        data["image"] = data.get("image") or data.pop("wiki_image", None)
        data.pop("wiki_image", None)
        _cache[mbid] = (time.time() + TTL, data)
    top = []
    for t in data["top"]:
        lt = library_track(name, t["title"])
        top.append({**t, "library": lt})
    return {**data, "top": top}
