"""YouTube Music: playlist import, account connection (likes/playlists sync) and song/album search for downloads.

Uses ytmusicapi (blocking `requests`), so every call runs in a worker thread.
"""
import asyncio
import json
import re
import time

from ytmusicapi import YTMusic
from ytmusicapi.auth.oauth import OAuthCredentials

from .. import config, db
from . import matcher

_anon: YTMusic | None = None


def anon() -> YTMusic:
    global _anon
    if _anon is None:
        _anon = YTMusic()
    return _anon


def _creds() -> OAuthCredentials | None:
    cid, sec = config.get("ytmusic_client_id"), config.get("ytmusic_client_secret")
    return OAuthCredentials(cid, sec) if cid and sec else None


def for_connection(conn: dict) -> YTMusic:
    d = conn["data"]
    if d.get("mode") == "browser":
        return YTMusic(json.dumps(d["headers"]))
    token = dict(d["token"])
    yt = YTMusic(token, oauth_credentials=_creds())
    return yt


def user_client(user_id: int | None) -> YTMusic:
    if user_id:
        from . import connections
        c = connections.get(user_id, "ytmusic")
        if c:
            return for_connection(c)
    return anon()


async def run(fn, *a, **k):
    return await asyncio.to_thread(fn, *a, **k)


# ---- connecting ------------------------------------------------------------------

async def oauth_start() -> dict:
    creds = _creds()
    if not creds:
        raise RuntimeError("The admin needs to add a YouTube Data API OAuth client (type “TVs and Limited Input devices”) in Settings → Integrations")
    code = await run(creds.get_code)
    return {"device_code": code["device_code"], "user_code": code["user_code"],
            "url": code.get("verification_url", "https://www.google.com/device"), "interval": code.get("interval", 5)}


async def oauth_check(user_id: int, device_code: str) -> dict:
    from . import connections
    creds = _creds()
    try:
        token = await run(creds.token_from_code, device_code)
    except Exception as e:  # noqa: BLE001
        raise RuntimeError(f"Google sign-in failed: {e}")
    if not token or "access_token" not in token:
        return {"done": False}
    token = dict(token)
    token.setdefault("expires_at", int(time.time()) + int(token.get("expires_in", 3600)))
    yt = YTMusic(token, oauth_credentials=creds)
    name = "YouTube Music"
    try:
        name = (await run(yt.get_account_info)).get("accountName") or name
    except Exception:  # noqa: BLE001
        pass
    connections.save(user_id, "ytmusic", name, {"mode": "oauth", "token": token})
    return {"done": True, "account": name}


async def connect_browser(user_id: int, headers_raw: str) -> dict:
    """Alternative to OAuth: paste the request headers of a music.youtube.com request (also unlocks uploads)."""
    from . import connections
    import ytmusicapi
    try:
        headers = json.loads(await run(ytmusicapi.setup, None, headers_raw))
    except Exception as e:  # noqa: BLE001
        raise RuntimeError(f"Couldn't read those headers: {e}")
    yt = YTMusic(json.dumps(headers))
    try:
        info = await run(yt.get_account_info)
    except Exception as e:  # noqa: BLE001
        raise RuntimeError(f"YouTube Music rejected those headers (copy them again from a fresh request): {e}")
    connections.save(user_id, "ytmusic", info.get("accountName") or "YouTube Music", {"mode": "browser", "headers": headers})
    return {"account": info.get("accountName")}


# ---- shapes ------------------------------------------------------------------------

def _t(x: dict, album: str | None = None) -> dict:
    from .sources import T
    arts = ", ".join(a.get("name", "") for a in (x.get("artists") or []) if a.get("name"))
    alb = (x.get("album") or {}).get("name") if isinstance(x.get("album"), dict) else (x.get("album") or album)
    thumbs = x.get("thumbnails") or []
    return T(x.get("title"), arts, alb, x.get("duration_seconds"), source_id=x.get("videoId"),
             cover=thumbs[-1]["url"] if thumbs else None)


def parse_id(url: str) -> tuple[str, str]:
    m = re.search(r"[?&]list=([\w-]+)", url)
    if m:
        return "playlist", m.group(1)
    m = re.search(r"/browse/(MPREb_[\w-]+)", url)
    if m:
        return "album", m.group(1)
    m = re.search(r"[?&]v=([\w-]{11})|youtu\.be/([\w-]{11})", url)
    if m:
        return "song", m.group(1) or m.group(2)
    raise ValueError("Not a YouTube Music playlist/album/song link")


async def fetch(url: str, user_id: int | None = None) -> dict:
    kind, pid = parse_id(url)
    yt = user_client(user_id)
    if kind == "playlist" and pid == "LM":
        if yt is anon():
            raise RuntimeError("Your liked songs need your YouTube Music account connected (Connections page)")
        d = await run(yt.get_liked_songs, 10000)
        return {"name": "YouTube Music Liked Songs", "description": "", "cover": None, "tracks": [_t(x) for x in d.get("tracks", [])]}
    if kind == "playlist" and pid.startswith("OLAK5uy_"):
        kind, pid = "album", await run(yt.get_album_browse_id, pid)
    if kind == "album":
        a = await run(yt.get_album, pid)
        return {"name": a.get("title"), "description": a.get("description") or "",
                "cover": (a.get("thumbnails") or [{}])[-1].get("url"), "tracks": [_t(x, a.get("title")) for x in a.get("tracks", [])]}
    if kind == "song":
        s = await run(yt.get_song, pid)
        v = s.get("videoDetails", {})
        from .sources import T
        return {"name": v.get("title"), "description": "", "cover": None,
                "tracks": [T(v.get("title"), v.get("author", "").replace(" - Topic", ""), None, float(v.get("lengthSeconds") or 0) or None, source_id=pid)]}
    p = await run(yt.get_playlist, pid, None)
    return {"name": p.get("title"), "description": p.get("description") or "",
            "cover": (p.get("thumbnails") or [{}])[-1].get("url"), "tracks": [_t(x) for x in p.get("tracks", []) if x.get("videoId")]}


# ---- search (downloads) -----------------------------------------------------------------

async def find_song(artist: str, title: str, length: float | None = None, user_id: int | None = None) -> dict | None:
    """Best official YouTube Music song (not a video/cover/remix unless asked for)."""
    want = matcher.describe(title, artist, length=length)
    try:
        res = await run(anon().search, f"{artist} {title}", "songs", None, 10)
    except Exception:  # noqa: BLE001
        return None
    best, best_s = None, 0.0
    for x in res:
        if not x.get("videoId"):
            continue
        cand = _t(x)
        v = matcher.compare(want, matcher.describe(cand["title"], cand["artist"], length=cand["duration"]))
        if v.match and v.score > best_s:
            best, best_s = {**cand, "videoId": x["videoId"], "album_id": (x.get("album") or {}).get("id")}, v.score
    return best


async def find_album(artist: str, album: str, track_count: int | None = None) -> dict | None:
    """→ {browseId, title, playlist_url, tracks} for the matching album."""
    want = matcher.describe(album, artist)
    try:
        res = await run(anon().search, f"{artist} {album}", "albums", None, 10)
    except Exception:  # noqa: BLE001
        return None
    for x in res:
        arts = ", ".join(a.get("name", "") for a in (x.get("artists") or []))
        if not matcher.compare(want, matcher.describe(x.get("title"), arts)).match:
            continue
        a = await run(anon().get_album, x["browseId"])
        n = len(a.get("tracks", []))
        if track_count and abs(n - track_count) > max(1, track_count // 5):
            continue
        pl = a.get("audioPlaylistId")
        return {"browseId": x["browseId"], "title": a.get("title"), "tracks": a.get("tracks", []),
                "playlist_url": f"https://music.youtube.com/playlist?list={pl}" if pl else None}
    return None


# ---- library track ↔ videoId mapping (for pushing likes/playlists) ------------------------

def _cache_get(track_id: int) -> str | None:
    r = db.one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", (f"ytm:{track_id}",))
    return r["value"] if r else None


def _cache_set(track_id: int, video_id: str) -> None:
    db.execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", (f"ytm:{track_id}", video_id))


async def video_for_track(track_id: int) -> str | None:
    vid = _cache_get(track_id)
    if vid:
        return vid if vid != "-" else None
    t = db.one("SELECT title, artist, duration FROM tracks WHERE id=?", (track_id,))
    if not t:
        return None
    hit = await find_song(t["artist"], t["title"], t["duration"])
    _cache_set(track_id, hit["videoId"] if hit else "-")
    return hit["videoId"] if hit else None
