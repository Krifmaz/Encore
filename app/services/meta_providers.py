"""Tag albums without MusicBrainz: Deezer, Apple Music and Discogs metadata, or AcoustID fingerprints.

Every provider returns releases in the same shape as musicbrainz.release(), so the normal
preview → retag flow (file mapping, renaming, cover art) works unchanged. Release ids carry
their provider: "deezer:123", "apple:456", "discogs:r789"; plain UUIDs are MusicBrainz.
"""
import asyncio
from collections import Counter

from .. import config
from . import musicbrainz as mb
from .http import client

PROVIDERS = {"musicbrainz": "MusicBrainz", "deezer": "Deezer", "apple": "Apple Music", "discogs": "Discogs", "fingerprint": "Audio fingerprint (AcoustID)"}


def _rel(id_, title, artist, date="", tracks=(), **kw) -> dict:
    tracks = list(tracks)
    return {"id": id_, "title": title, "artist": artist, "artist_id": None, "date": date or "", "year": (date or "")[:4],
            "country": kw.get("country", ""), "status": "Official", "release_group_id": None, "type": kw.get("type", ""),
            "label": kw.get("label", ""), "catalog": kw.get("catalog", ""), "barcode": kw.get("barcode", ""),
            "disc_count": len({t["disc"] for t in tracks}) or 1, "genre": kw.get("genre", ""), "tracks": tracks,
            "cover": kw.get("cover"), "cover_url": kw.get("cover_big") or kw.get("cover"), "provider": kw.get("provider")}


def _t(title, artist, track, disc=1, length=None, isrc=None) -> dict:
    return {"title": title or "", "artist": artist or "", "artist_id": None, "track": int(track or 0), "disc": int(disc or 1),
            "length": float(length or 0), "recording_id": None, "track_id": None, "isrc": isrc}


async def search(provider: str, artist: str, album: str, track_count: int, paths: list[str] | None = None) -> list[dict]:
    if provider == "deezer":
        r = await client().get("https://api.deezer.com/search/album", params={"q": f'artist:"{artist}" album:"{album}"' if artist else album, "limit": 20})
        data = r.json().get("data", []) or (await client().get("https://api.deezer.com/search/album", params={"q": f"{artist} {album}", "limit": 20})).json().get("data", [])
        return [{"id": f"deezer:{a['id']}", "title": a.get("title", ""), "artist": (a.get("artist") or {}).get("name", ""), "date": a.get("release_date", ""),
                 "country": "", "track_count": a.get("nb_tracks") or 0, "formats": "Digital", "type": (a.get("record_type") or "").title(),
                 "label": "", "score": 90, "cover": a.get("cover_medium"), "provider": "Deezer"} for a in data]
    if provider == "apple":
        r = await client().get("https://itunes.apple.com/search", params={"term": f"{artist} {album}", "entity": "album", "limit": 20})
        return [{"id": f"apple:{a['collectionId']}", "title": a.get("collectionName", ""), "artist": a.get("artistName", ""), "date": (a.get("releaseDate") or "")[:10],
                 "country": a.get("country", ""), "track_count": a.get("trackCount") or 0, "formats": "Digital", "type": "",
                 "label": (a.get("copyright") or "")[:60], "score": 90, "cover": (a.get("artworkUrl100") or "").replace("100x100bb", "300x300bb"), "provider": "Apple Music"}
                for a in r.json().get("results", []) if a.get("collectionId")]
    if provider == "discogs":
        from .catalog import DISCOGS, _discogs_headers
        if not config.get("discogs_token"):
            raise ValueError("Add a Discogs token in Settings to search Discogs")
        r = await client().get(f"{DISCOGS}/database/search", params={"artist": artist, "release_title": album, "type": "release", "per_page": 20}, headers=_discogs_headers())
        return [{"id": f"discogs:r{x['id']}", "title": x.get("title", ""), "artist": "", "date": str(x.get("year") or ""), "country": x.get("country", ""),
                 "track_count": 0, "formats": ", ".join(x.get("format") or []), "type": "", "label": ", ".join((x.get("label") or [])[:2]),
                 "score": 80, "cover": x.get("thumb"), "provider": "Discogs"} for x in r.json().get("results", [])]
    if provider == "fingerprint":
        return await _by_fingerprint(paths or [], track_count)
    out = await mb.search_releases(artist if artist != "Unknown Artist" else "", album, 20)
    for r in out:
        r["provider"] = "MusicBrainz"
    return out


async def _by_fingerprint(paths: list[str], n: int) -> list[dict]:
    """Identify the songs by their audio (names don't matter), then find the releases most of them are on."""
    from . import acoustid
    if not acoustid.available():
        raise ValueError("Audio fingerprinting needs an AcoustID key (Settings) and fpcalc/ffmpeg")
    votes: Counter = Counter()
    for p in paths[:30]:
        try:
            res = await acoustid.identify(p)
        except Exception:  # noqa: BLE001
            continue
        best = (res or {}).get("results") or []
        if not best or best[0]["score"] < 0.7:
            continue
        rgs = {g for rec in best[0]["recordings"] for g in rec.get("release_group_ids", [])}
        votes.update(rgs)
    out = []
    for rgid, hits in votes.most_common(6):
        rid = await mb.best_release_for_group(rgid, n)
        if not rid:
            continue
        rel = await mb.release(rid)
        out.append({"id": rid, "title": rel["title"], "artist": rel["artist"], "date": rel["date"], "country": rel["country"],
                    "track_count": len(rel["tracks"]), "formats": "", "type": rel["type"], "label": rel["label"],
                    "score": round(100 * hits / max(len(paths), 1)), "cover": rel.get("cover"), "provider": f"Fingerprint · {hits}/{len(paths)} songs"})
    return out


async def release(release_id: str) -> dict:
    kind, _, rid = release_id.partition(":")
    if kind == "deezer":
        a = (await client().get(f"https://api.deezer.com/album/{rid}")).json()
        tr = (await client().get(f"https://api.deezer.com/album/{rid}/tracks", params={"limit": 500})).json().get("data", [])
        isrcs = {}
        if tr and not tr[0].get("isrc"):
            sem = asyncio.Semaphore(5)

            async def one(t):
                async with sem:
                    try:
                        isrcs[t["id"]] = (await client().get(f"https://api.deezer.com/track/{t['id']}")).json().get("isrc")
                    except Exception:  # noqa: BLE001
                        pass
            await asyncio.gather(*[one(t) for t in tr[:60]])
        genres = [g.get("name") for g in ((a.get("genres") or {}).get("data") or [])]
        return _rel(release_id, a.get("title", ""), (a.get("artist") or {}).get("name", ""), a.get("release_date", ""),
                    [_t(t.get("title"), (t.get("artist") or {}).get("name"), t.get("track_position") or i, t.get("disk_number"), t.get("duration"),
                        t.get("isrc") or isrcs.get(t.get("id"))) for i, t in enumerate(tr, 1)],
                    label=a.get("label", ""), barcode=a.get("upc", ""), genre=genres[0] if genres else "", type=(a.get("record_type") or "").title(),
                    cover=a.get("cover_medium"), cover_big=a.get("cover_xl"), provider="Deezer")
    if kind == "apple":
        r = (await client().get("https://itunes.apple.com/lookup", params={"id": rid, "entity": "song", "limit": 300})).json().get("results", [])
        col = next((x for x in r if x.get("wrapperType") == "collection"), {})
        tracks = [_t(x.get("trackName"), x.get("artistName"), x.get("trackNumber"), x.get("discNumber"), (x.get("trackTimeMillis") or 0) / 1000)
                  for x in r if x.get("wrapperType") == "track"]
        art = (col.get("artworkUrl100") or "")
        return _rel(release_id, col.get("collectionName", ""), col.get("artistName", ""), (col.get("releaseDate") or "")[:10], tracks,
                    genre=col.get("primaryGenreName", ""), label=(col.get("copyright") or "")[:80], country=col.get("country", ""),
                    cover=art.replace("100x100bb", "500x500bb"), cover_big=art.replace("100x100bb", "3000x3000bb"), provider="Apple Music")
    if kind == "discogs":
        from .catalog import DISCOGS, _dur, _discogs_headers
        path = ("masters/" if rid.startswith("m") else "releases/") + rid[1:]
        d = (await client().get(f"{DISCOGS}/{path}", headers=_discogs_headers())).json()
        artist = ", ".join(a.get("name", "") for a in d.get("artists") or []).replace(" (", " ").rstrip(")") if d.get("artists") else ""
        tl = [t for t in d.get("tracklist", []) if t.get("type_", "track") == "track"]
        tracks = [_t(t.get("title"), ", ".join(a.get("name", "") for a in t.get("artists") or []) or artist, i, 1, _dur(t.get("duration")))
                  for i, t in enumerate(tl, 1)]
        img = next((i.get("uri") for i in d.get("images") or [] if i.get("type") == "primary"), None) or next((i.get("uri") for i in d.get("images") or []), None)
        barcode = next((i.get("value") for i in d.get("identifiers") or [] if i.get("type") == "Barcode"), "")
        return _rel(release_id, d.get("title", ""), artist, str(d.get("released") or d.get("year") or ""), tracks,
                    label=", ".join(lbl.get("name", "") for lbl in (d.get("labels") or [])[:2]), catalog=", ".join(lbl.get("catno", "") for lbl in (d.get("labels") or [])[:2]),
                    barcode=barcode, country=d.get("country", ""), genre=(d.get("genres") or d.get("styles") or [""])[0], cover=img, cover_big=img, provider="Discogs")
    rel = await mb.release(release_id)
    rel["provider"] = "MusicBrainz"
    return rel
