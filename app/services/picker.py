"""Manual download picker ("interactive search"): list what each source has, let the user choose."""
import re

from .. import config
from . import lidarr, matcher, slskd, streamrip, ytmusic
from .http import client

LOSSLESS = {"flac", "alac", "wav", "aiff", "aif", "ape", "wv"}


def available() -> list[dict]:
    out = []
    if slskd.enabled():
        out.append({"id": "slskd", "name": "Soulseek"})
    if streamrip.enabled() and "deezer" in config.get("streamrip_sources"):
        out.append({"id": "deezer", "name": "Deezer"})
    if config.get("ytdlp_enabled"):
        out.append({"id": "ytmusic", "name": "YouTube Music"})
    if lidarr.enabled():
        out.append({"id": "lidarr", "name": "Lidarr"})
    return out


def _quality(fmt: str, bitrate: int = 0, depth: int = 0, rate: int = 0) -> str:
    fmt = (fmt or "").lower()
    if fmt in LOSSLESS:
        bits = f" {depth}-bit" if depth else ""
        khz = f"/{rate / 1000:g}kHz" if rate and rate != 44100 else ""
        return f"{fmt.upper()}{bits}{khz}"
    return f"{fmt.upper()} {bitrate}k" if bitrate else fmt.upper()


def _match(want_title: str, want_artist: str, title: str, artist: str, length=None, got_length=None) -> float:
    try:
        v = matcher.compare(matcher.describe(want_title, want_artist, length=length), matcher.describe(title, artist, length=got_length))
        return round(v.score, 2) if v.match else 0.0
    except Exception:  # noqa: BLE001
        return 0.0


async def search_slskd(kind: str, artist: str, title: str, query: str, track_count: int | None, length: float | None) -> list[dict]:
    responses = await slskd.search(query)
    out = []
    for resp in responses:
        meta = {"user": resp.get("username"), "free_slot": bool(resp.get("hasFreeUploadSlot")),
                "queue": resp.get("queueLength", 0), "speed": resp.get("uploadSpeed") or 0}
        audio = [f for f in resp.get("files", []) if slskd._ext(f["filename"]) in slskd.AUDIO_EXT]
        if kind == "album":
            dirs: dict[str, list] = {}
            for f in audio:
                dirs.setdefault(f["filename"].replace("\\", "/").rsplit("/", 1)[0], []).append(f)
            groups = [(d, fs) for d, fs in dirs.items()]
        else:
            groups = [(f["filename"].replace("\\", "/"), [f]) for f in audio]
        for d, files in groups:
            exts = {slskd._ext(f["filename"]) for f in files}
            fmt = exts.pop() if len(exts) == 1 else "mixed"
            name = d.split("/")[-1] if kind == "track" else "/".join(d.split("/")[-2:])
            rel = slskd._contains(name, title)
            if rel < 0.4:
                continue
            br = min((f.get("bitRate") or 0) for f in files)
            depth = min((f.get("bitDepth") or 0) for f in files)
            rate = max((f.get("sampleRate") or 0) for f in files)
            score = rel * 40 + slskd._contains(d, artist) * 10 + (30 if fmt in LOSSLESS else 15 if br >= 320 else 5)
            score += (6 if depth >= 24 else 0) + (8 if meta["free_slot"] else 0) - min(meta["queue"], 50) / 5
            if kind == "album" and track_count:
                score += 15 if len(files) == track_count else -10 if len(files) < track_count else 0
            if kind == "track" and length and files[0].get("length"):
                score += 10 if abs(files[0]["length"] - length) < 5 else -10
            out.append({
                "source": "slskd", "key": f"{meta['user']}|{d}",
                "title": name, "subtitle": d.replace("/", " / "),
                "quality": _quality(fmt, br, depth, rate), "lossless": fmt in LOSSLESS,
                "tracks": len(files), "size": sum(f.get("size", 0) for f in files),
                "duration": files[0].get("length") if kind == "track" else None,
                "info": " · ".join(filter(None, [meta["user"], "free slot" if meta["free_slot"] else f"queue {meta['queue']}",
                                                 f"{meta['speed'] / 1_000_000:.1f} MB/s" if meta["speed"] else ""])),
                "score": round(score, 1),
                "payload": {"username": meta["user"], "directory": d.rsplit("/", 1)[0] if kind == "track" else d,
                            "files": [{"filename": f["filename"], "size": f["size"]} for f in sorted(files, key=lambda x: x["filename"])],
                            "format": fmt, "bitrate": br},
            })
    out.sort(key=lambda c: -c["score"])
    return out[:150]


async def search_deezer(kind: str, artist: str, title: str, query: str, track_count: int | None, length: float | None) -> list[dict]:
    r = await client().get(f"https://api.deezer.com/search/{'album' if kind == 'album' else 'track'}", params={"q": query, "limit": 25})
    out = []
    for x in r.json().get("data", []):
        a = (x.get("artist") or {}).get("name", "")
        if kind == "album":
            out.append({"source": "deezer", "key": f"deezer:album:{x['id']}", "title": x.get("title", ""), "subtitle": a,
                        "quality": "FLAC (Deezer HiFi)", "lossless": True, "tracks": x.get("nb_tracks"), "size": None,
                        "info": " · ".join(filter(None, [(x.get("record_type") or "").title(), "explicit" if x.get("explicit_lyrics") else ""])),
                        "cover": x.get("cover_medium"), "score": _match(title, artist, x.get("title", ""), a),
                        "payload": {"url": f"https://www.deezer.com/album/{x['id']}"}})
        else:
            t = x.get("title", "") + (f" {x['title_version']}" if x.get("title_version") and x["title_version"].lower() not in x.get("title", "").lower() else "")
            out.append({"source": "deezer", "key": f"deezer:track:{x['id']}", "title": t, "subtitle": f"{a} · {(x.get('album') or {}).get('title', '')}",
                        "quality": "FLAC (Deezer HiFi)", "lossless": True, "tracks": 1, "size": None, "duration": x.get("duration"),
                        "info": "explicit" if x.get("explicit_lyrics") else "", "cover": (x.get("album") or {}).get("cover_medium"),
                        "score": _match(title, artist, t, a, length, x.get("duration")),
                        "payload": {"url": f"https://www.deezer.com/track/{x['id']}"}})
    out.sort(key=lambda c: -c["score"])
    return out


async def search_ytmusic(kind: str, artist: str, title: str, query: str, track_count: int | None, length: float | None) -> list[dict]:
    res = await ytmusic.run(ytmusic.anon().search, query, "albums" if kind == "album" else "songs", None, 20)
    out = []
    for x in res:
        arts = ", ".join(a.get("name", "") for a in (x.get("artists") or []) if a.get("id") or a.get("name") not in ("Album", "Single", "EP"))
        thumb = (x.get("thumbnails") or [{}])[-1].get("url")
        if kind == "album" and x.get("browseId"):
            out.append({"source": "ytmusic", "key": f"ytm:{x['browseId']}", "title": x.get("title", ""), "subtitle": arts,
                        "quality": "Opus ~160k / AAC 256k", "lossless": False, "tracks": None, "size": None,
                        "info": " · ".join(filter(None, [x.get("type"), str(x.get("year") or "")])), "cover": thumb,
                        "score": _match(title, artist, x.get("title", ""), arts),
                        "payload": {"browseId": x["browseId"]}})
        elif kind == "track" and x.get("videoId"):
            dur = x.get("duration_seconds")
            out.append({"source": "ytmusic", "key": f"ytm:{x['videoId']}", "title": x.get("title", ""),
                        "subtitle": f"{arts} · {(x.get('album') or {}).get('name', '')}".rstrip(" ·"),
                        "quality": "Opus ~160k / AAC 256k", "lossless": False, "tracks": 1, "size": None, "duration": dur,
                        "info": "explicit" if x.get("isExplicit") else "", "cover": thumb,
                        "score": _match(title, artist, x.get("title", ""), arts, length, dur),
                        "payload": {"url": f"https://music.youtube.com/watch?v={x['videoId']}"}})
    return out


async def search_lidarr(kind: str, artist: str, title: str, mbid: str | None, artist_mbid: str | None) -> list[dict]:
    if kind != "album" or not mbid:
        raise ValueError("Lidarr searches whole albums only")
    alb = await lidarr.album_by_rg(mbid)
    if not alb:
        if artist_mbid:
            a = await lidarr.ensure_artist(artist_mbid)
            await lidarr.refresh_artist(a["id"])
        raise ValueError("Lidarr is still loading this artist – try again in a minute")
    rels = await lidarr._req("GET", "release", params={"albumId": alb["id"]}) or []
    out = []
    for x in rels:
        q = ((x.get("quality") or {}).get("quality") or {}).get("name", "")
        seeds = x.get("seeders")
        out.append({"source": "lidarr", "key": f"lidarr:{x.get('guid')}", "title": x.get("title", ""), "subtitle": x.get("indexer", ""),
                    "quality": q, "lossless": bool(re.search(r"(?i)flac|alac|lossless", q)), "tracks": None, "size": x.get("size"),
                    "info": " · ".join(filter(None, [x.get("protocol", ""), f"{seeds} seeders" if seeds is not None else "", f"{x.get('ageHours', 0) / 24:.0f} days old" if x.get("ageHours") else ""])),
                    "warning": "; ".join(x.get("rejections") or []), "score": (100 if x.get("approved") else 0) + (seeds or 0) / 10,
                    "payload": {"guid": x.get("guid"), "indexerId": x.get("indexerId"), "album_id": alb["id"]}})
    out.sort(key=lambda c: -c["score"])
    return out


async def search(source: str, kind: str, artist: str, title: str, query: str | None = None, mbid: str | None = None,
                 track_count: int | None = None, length: float | None = None, artist_mbid: str | None = None) -> list[dict]:
    q = (query or "").strip() or " ".join(filter(None, [artist, title]))
    if source == "slskd":
        return await search_slskd(kind, artist, title, re.sub(r"[^\w\s']", " ", q), track_count, length)
    if source == "deezer":
        return await search_deezer(kind, artist, title, q, track_count, length)
    if source == "ytmusic":
        return await search_ytmusic(kind, artist, title, q, track_count, length)
    if source == "lidarr":
        return await search_lidarr(kind, artist, title, mbid, artist_mbid)
    raise ValueError(f"Unknown source {source}")


async def resolve(source: str, payload: dict) -> dict:
    """Turn a picked result into what the worker needs (YouTube Music albums → playlist url)."""
    if source == "ytmusic" and payload.get("browseId"):
        a = await ytmusic.run(ytmusic.anon().get_album, payload["browseId"])
        pl = a.get("audioPlaylistId")
        if not pl:
            raise ValueError("YouTube Music has no playable playlist for this album")
        return {"url": f"https://music.youtube.com/playlist?list={pl}", "title": a.get("title")}
    return payload
