"""Cover art candidates from every catalogue, for picking artwork by hand."""
import asyncio

from .. import config
from .http import client


async def candidates(artist: str, album: str, rgid: str | None = None, release_id: str | None = None) -> list[dict]:
    out: list[dict] = []

    async def caa():
        from . import musicbrainz as mb
        ids = []
        if release_id:
            ids.append(("release", release_id))
        if rgid:
            ids.append(("release-group", rgid))
        if not ids:
            for rg in (await mb.search_albums(f'release:"{album}" AND artist:"{artist}"', 4))[:4]:
                ids.append(("release-group", rg["id"]))
        res = []
        for kind, i in ids[:4]:
            r = await client().get(f"https://coverartarchive.org/{kind}/{i}", follow_redirects=True)
            if r.status_code == 200:
                for img in r.json().get("images", [])[:6]:
                    if img.get("front") or "Front" in (img.get("types") or []):
                        res.append({"url": img["image"], "thumb": (img.get("thumbnails") or {}).get("500") or img["image"],
                                    "source": "Cover Art Archive", "label": kind.replace("-", " ")})
        return res

    async def deezer():
        r = await client().get("https://api.deezer.com/search/album", params={"q": f"{artist} {album}", "limit": 8})
        return [{"url": a.get("cover_xl"), "thumb": a.get("cover_medium"), "source": "Deezer", "label": f"{(a.get('artist') or {}).get('name', '')} – {a.get('title', '')}", "size": "1000×1000"}
                for a in r.json().get("data", []) if a.get("cover_xl")]

    async def apple():
        r = await client().get("https://itunes.apple.com/search", params={"term": f"{artist} {album}", "entity": "album", "limit": 8})
        return [{"url": a["artworkUrl100"].replace("100x100bb", "3000x3000bb"), "thumb": a["artworkUrl100"].replace("100x100bb", "300x300bb"),
                 "source": "Apple Music", "label": f"{a.get('artistName', '')} – {a.get('collectionName', '')}", "size": "up to 3000×3000"}
                for a in r.json().get("results", []) if a.get("artworkUrl100")]

    async def ytm():
        from . import ytmusic
        res = await ytmusic.run(ytmusic.anon().search, f"{artist} {album}", "albums", None, 6)
        out = []
        for a in res:
            th = (a.get("thumbnails") or [{}])[-1].get("url")
            if th:
                big = th.split("=w")[0] + "=w1200-h1200-l90-rj" if "=w" in th else th
                out.append({"url": big, "thumb": th, "source": "YouTube Music", "label": a.get("title", ""), "size": "1200×1200"})
        return out

    async def discogs():
        if not config.get("discogs_token"):
            return []
        from .catalog import DISCOGS, _discogs_headers
        r = await client().get(f"{DISCOGS}/database/search", params={"artist": artist, "release_title": album, "type": "release", "per_page": 6},
                               headers=_discogs_headers())
        return [{"url": x["cover_image"], "thumb": x.get("thumb") or x["cover_image"], "source": "Discogs", "label": x.get("title", "")}
                for x in r.json().get("results", []) if x.get("cover_image") and "spacer.gif" not in x["cover_image"]]

    for res in await asyncio.gather(caa(), deezer(), apple(), ytm(), discogs(), return_exceptions=True):
        if isinstance(res, list):
            out += res
    seen, uniq = set(), []
    for c in out:
        if c["url"] and c["url"] not in seen:
            seen.add(c["url"])
            uniq.append(c)
    return uniq
