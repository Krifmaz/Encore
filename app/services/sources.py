"""Read playlists / albums / liked songs from streaming services into a common shape.

Every fetcher returns {"name", "description", "cover", "source", "tracks": [Track]} where Track is
{"title", "artist", "album", "duration", "isrc", "mbid", "source_id", "cover"}.
"""
import asyncio
import base64
import json
import re
import time
from html import unescape

from .. import config
from .http import client

UA = {"User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36",
      "Accept-Language": "en-US,en;q=0.9"}


class SourceError(Exception):
    pass


def T(title, artist, album=None, duration=None, isrc=None, mbid=None, source_id=None, cover=None) -> dict:
    return {"title": (title or "").strip(), "artist": (artist or "").strip(), "album": album, "duration": duration,
            "isrc": (isrc or "").upper().replace("-", "") or None, "mbid": mbid, "source_id": source_id, "cover": cover}


def detect(url: str) -> str | None:
    u = url.strip()
    rules = [
        ("spotify", r"open\.spotify\.com|^spotify:|spotify\.link"),
        ("deezer", r"deezer\.com|deezer\.page\.link|link\.deezer\.com"),
        ("tidal", r"tidal\.com"),
        ("qobuz", r"qobuz\.com"),
        ("apple", r"music\.apple\.com"),
        ("listenbrainz", r"listenbrainz\.org/playlist"),
        ("ytmusic", r"music\.youtube\.com"),
        ("ytdlp", r"youtube\.com|youtu\.be|soundcloud\.com|bandcamp\.com|mixcloud\.com|audiomack\.com|vimeo\.com"),
    ]
    for name, pat in rules:
        if re.search(pat, u, re.I):
            return name
    return "web" if u.startswith("http") else None


async def fetch(url: str, user_id: int | None = None) -> dict:
    kind = detect(url)
    fn = {"spotify": spotify, "ytmusic": ytmusic_fetch, "deezer": deezer, "tidal": tidal, "qobuz": web_jsonld, "apple": apple,
          "listenbrainz": listenbrainz, "ytdlp": ytdlp_playlist, "web": web_jsonld}.get(kind)
    if not fn:
        raise SourceError("Unsupported link")
    data = await fn(url, user_id)
    data["source"] = kind if kind != "web" else "web"
    data["tracks"] = [t for t in data["tracks"] if t["title"]]
    if not data["tracks"]:
        raise SourceError("No tracks found at that link (is the playlist public?)")
    return data


def parse_text(text: str) -> dict:
    """'Artist - Title' (or 'Title by Artist', or CSV artist,title) per line."""
    tracks = []
    for line in text.splitlines():
        line = line.strip().strip("﻿")
        if not line or line.startswith("#"):
            continue
        line = re.sub(r"^\d+[\.\)]\s*", "", line)
        if " - " in line or " – " in line or " — " in line:
            artist, title = re.split(r"\s[-–—]\s", line, maxsplit=1)
        elif " by " in line.lower():
            title, artist = re.split(r"\sby\s", line, maxsplit=1, flags=re.I)
        elif "," in line:
            artist, title = [x.strip().strip('"') for x in line.split(",", 1)]
        elif "\t" in line:
            artist, title = line.split("\t", 1)
        else:
            continue
        tracks.append(T(title, artist))
    if not tracks:
        raise SourceError("Couldn't read any 'Artist - Title' lines")
    return {"name": "Imported list", "description": "", "cover": None, "source": "text", "tracks": tracks}


# ---- Spotify -----------------------------------------------------------------

def _spotify_id(url: str) -> tuple[str, str]:
    m = re.search(r"(?:open\.spotify\.com/(?:intl-[\w-]+/)?(?:user/[^/]+/)?|spotify:)(playlist|album|track|artist|collection)[/:]?([A-Za-z0-9]*)", url)
    if not m:
        raise SourceError("Not a Spotify playlist/album/track link")
    return m.group(1), m.group(2)


async def _resolve_short(url: str) -> str:
    if re.search(r"spotify\.link|deezer\.page\.link|link\.deezer\.com", url):
        r = await client().get(url, headers=UA, follow_redirects=True)
        return str(r.url)
    return url


def _sp_track(t: dict) -> dict:
    alb = t.get("album") or {}
    imgs = alb.get("images") or []
    return T(t.get("name"), ", ".join(a["name"] for a in t.get("artists", [])), alb.get("name"),
             (t.get("duration_ms") or 0) / 1000 or None, (t.get("external_ids") or {}).get("isrc"),
             source_id=t.get("id"), cover=imgs[0]["url"] if imgs else None)


async def spotify_api(token: str, kind: str, sid: str) -> dict:
    h = {"Authorization": f"Bearer {token}"}
    c = client()
    base = "https://api.spotify.com/v1"

    async def get(url):
        r = await c.get(url, headers=h)
        if r.status_code == 429:
            await asyncio.sleep(int(r.headers.get("Retry-After", "2")))
            r = await c.get(url, headers=h)
        if r.status_code >= 400:
            raise SourceError(f"Spotify API {r.status_code}: {r.text[:200]}")
        return r.json()

    if kind == "track":
        t = await get(f"{base}/tracks/{sid}")
        return {"name": t["name"], "description": "", "cover": None, "tracks": [_sp_track(t)]}
    if kind == "collection":  # Liked Songs
        items, nxt = [], f"{base}/me/tracks?limit=50"
        while nxt and len(items) < 10000:
            page = await get(nxt)
            items += [_sp_track(i["track"]) for i in page.get("items", []) if i.get("track")]
            nxt = page.get("next")
        return {"name": "Spotify Liked Songs", "description": "", "cover": None, "tracks": items}
    meta = await get(f"{base}/{kind}s/{sid}")
    imgs = meta.get("images") or []
    items = []
    if kind == "album":
        for t in (meta.get("tracks") or {}).get("items", []):
            items.append(_sp_track({**t, "album": meta}))
        nxt = (meta.get("tracks") or {}).get("next")
    else:
        # Feb-2026 API renamed /tracks → /items; older apps still have /tracks
        nxt = f"{base}/playlists/{sid}/items?limit=100"
        try:
            await get(nxt)
        except SourceError:
            nxt = f"{base}/playlists/{sid}/tracks?limit=100"
    while nxt and len(items) < 10000:
        page = await get(nxt)
        for it in page.get("items", []):
            t = (it.get("item") or it.get("track")) if kind == "playlist" else it
            if t and t.get("name") and t.get("type", "track") == "track":
                items.append(_sp_track(t if kind == "playlist" else {**t, "album": meta}))
        nxt = page.get("next")
    return {"name": meta.get("name"), "description": unescape(meta.get("description") or ""),
            "cover": imgs[0]["url"] if imgs else None, "tracks": items}


async def _spotify_app_token() -> str | None:
    cid, sec = config.get("spotify_client_id"), config.get("spotify_client_secret")
    if not (cid and sec):
        return None
    r = await client().post("https://accounts.spotify.com/api/token", data={"grant_type": "client_credentials"}, auth=(cid, sec))
    return r.json().get("access_token") if r.status_code == 200 else None


async def spotify_embed(kind: str, sid: str) -> dict:
    """No-credentials fallback: Spotify's public embed player ships the track list as JSON."""
    r = await client().get(f"https://open.spotify.com/embed/{kind}/{sid}", headers=UA)
    if r.status_code != 200:
        raise SourceError(f"Spotify embed page returned {r.status_code}")
    m = re.search(r'<script id="__NEXT_DATA__" type="application/json">(.*?)</script>', r.text, re.S)
    if not m:
        raise SourceError("Spotify embed page format changed")
    data = json.loads(m.group(1))
    entity = _walk_find(data, lambda d: isinstance(d, dict) and "trackList" in d) or {}
    tracks = []
    for t in entity.get("trackList", []):
        tracks.append(T(t.get("title"), t.get("subtitle", "").replace(" ", " "), None,
                        (t.get("duration") or 0) / 1000 or None, source_id=(t.get("uri") or "").split(":")[-1]))
    if not tracks and entity.get("title"):  # single track embed
        tracks.append(T(entity.get("title"), ", ".join(a.get("name", "") for a in entity.get("artists", [])), None,
                        (entity.get("duration") or 0) / 1000 or None, source_id=sid))
    imgs = ((entity.get("coverArt") or {}).get("sources") or [{}])
    return {"name": entity.get("name") or entity.get("title") or "Spotify import", "description": "",
            "cover": imgs[0].get("url"), "tracks": tracks}


async def spotify(url: str, user_id: int | None = None) -> dict:
    url = await _resolve_short(url)
    kind, sid = _spotify_id(url)
    from . import connections
    token = await connections.spotify_token(user_id) if user_id else None
    errors = []
    for tok in (token, await _spotify_app_token() if kind != "collection" else None):
        if not tok:
            continue
        try:
            return await spotify_api(tok, kind, sid)
        except SourceError as e:
            errors.append(str(e))
    if kind == "collection":
        raise SourceError("Liked Songs need your Spotify account connected (Connections page)")
    try:
        data = await spotify_embed(kind, sid)
        data["description"] = "Imported without Spotify login – connect Spotify for ISRCs and >100-track playlists"
        return data
    except SourceError as e:
        errors.append(str(e))
    raise SourceError("; ".join(errors))


# ---- Deezer (public API, no key needed) ----------------------------------------

async def deezer(url: str, user_id: int | None = None) -> dict:
    url = await _resolve_short(url)
    m = re.search(r"deezer\.com/(?:[a-z]{2}/)?(playlist|album|track|profile)/(\d+)(?:/(loved|playlists))?", url)
    if not m:
        raise SourceError("Not a Deezer playlist/album/track link")
    kind, did = m.group(1), m.group(2)
    c = client()
    api = "https://api.deezer.com"

    async def get(u):
        r = await c.get(u)
        d = r.json()
        if isinstance(d, dict) and d.get("error"):
            raise SourceError(f"Deezer: {d['error'].get('message')}")
        return d

    def dz(t, album=None):
        alb = t.get("album") or album or {}
        title = t.get("title") or ""
        ver = t.get("title_version") or ""
        if ver and ver.lower() not in title.lower():
            title = f"{title} {ver}"
        return T(title, (t.get("artist") or {}).get("name"), alb.get("title"), t.get("duration"),
                 t.get("isrc"), source_id=str(t.get("id")), cover=alb.get("cover_xl") or alb.get("cover_big"))

    if kind == "track":
        t = await get(f"{api}/track/{did}")
        return {"name": t["title"], "description": "", "cover": None, "tracks": [dz(t)]}
    if kind == "profile":
        meta = await get(f"{api}/user/{did}")
        nxt, name = f"{api}/user/{did}/tracks?limit=100", f"{meta.get('name')}'s Deezer favourites"
        cover = meta.get("picture_xl")
        album = None
    else:
        meta = await get(f"{api}/{kind}/{did}")
        nxt, name = f"{api}/{kind}/{did}/tracks?limit=100", meta.get("title")
        cover = meta.get("picture_xl") or meta.get("cover_xl")
        album = meta if kind == "album" else None
    tracks = []
    while nxt and len(tracks) < 10000:
        page = await get(nxt)
        tracks += [dz(t, album) for t in page.get("data", [])]
        nxt = page.get("next")
    return {"name": name, "description": meta.get("description") or "", "cover": cover, "tracks": tracks}


async def deezer_isrc(isrc: str) -> dict | None:
    """ISRC → Deezer track (used to pick an exact lossless source for streamrip)."""
    r = await client().get(f"https://api.deezer.com/track/isrc:{isrc}")
    d = r.json()
    return None if d.get("error") else d


# ---- Tidal (developer API, client credentials) -----------------------------------

_tidal_tok = {"v": None, "exp": 0}


async def _tidal_token() -> str:
    if _tidal_tok["v"] and _tidal_tok["exp"] > time.time() + 30:
        return _tidal_tok["v"]
    cid, sec = config.get("tidal_client_id"), config.get("tidal_client_secret")
    if not (cid and sec):
        raise SourceError("Tidal import needs a Client ID/Secret from developer.tidal.com (Settings → Integrations)")
    r = await client().post("https://auth.tidal.com/v1/oauth2/token", data={"grant_type": "client_credentials"},
                            headers={"Authorization": "Basic " + base64.b64encode(f"{cid}:{sec}".encode()).decode()})
    if r.status_code != 200:
        raise SourceError(f"Tidal auth failed: {r.text[:200]}")
    d = r.json()
    _tidal_tok.update(v=d["access_token"], exp=time.time() + d.get("expires_in", 3600))
    return _tidal_tok["v"]


def _iso_dur(s: str | None) -> float | None:
    if not s:
        return None
    m = re.match(r"P(?:T)?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?", s)
    return (int(m.group(1) or 0) * 3600 + int(m.group(2) or 0) * 60 + float(m.group(3) or 0)) if m else None


async def tidal_api(token: str, kind: str, tid: str, country: str = "US") -> dict:
    h = {"Authorization": f"Bearer {token}", "Accept": "application/vnd.api+json"}
    c = client()
    base = "https://openapi.tidal.com/v2"

    async def get(path, params=None):
        r = await c.get(path if path.startswith("http") else base + path, headers=h, params=params)
        if r.status_code == 429:
            await asyncio.sleep(2)
            r = await c.get(path if path.startswith("http") else base + path, headers=h, params=params)
        if r.status_code >= 400:
            raise SourceError(f"Tidal API {r.status_code}: {r.text[:200]}")
        return r.json()

    meta = await get(f"/{kind}s/{tid}", {"countryCode": country})
    attrs = (meta.get("data") or {}).get("attributes") or {}
    ids, nxt = [], f"/{kind}s/{tid}/relationships/items" if kind != "track" else None
    if kind == "track":
        ids = [tid]
    params = {"countryCode": country}
    while nxt and len(ids) < 10000:
        page = await get(nxt, params)
        ids += [d["id"] for d in page.get("data", []) if d.get("type") == "tracks"]
        nxt = (page.get("links") or {}).get("next")
        if nxt and not nxt.startswith("http"):
            nxt = base + nxt if nxt.startswith("/") else nxt
        params = None
    tracks = []
    for i in range(0, len(ids), 20):
        chunk = ids[i:i + 20]
        d = await get("/tracks", {"countryCode": country, "filter[id]": ",".join(chunk), "include": "artists,albums"})
        inc = {(x["type"], x["id"]): x.get("attributes", {}) for x in d.get("included", [])}
        by_id = {x["id"]: x for x in d.get("data", [])}
        for tid_ in chunk:
            x = by_id.get(tid_)
            if not x:
                continue
            a = x.get("attributes", {})
            rel = x.get("relationships", {})
            artists = [inc.get(("artists", r["id"]), {}).get("name", "") for r in (rel.get("artists") or {}).get("data", [])]
            albums = [inc.get(("albums", r["id"]), {}).get("title") for r in (rel.get("albums") or {}).get("data", [])]
            title = a.get("title", "")
            if a.get("version") and a["version"].lower() not in title.lower():
                title = f"{title} ({a['version']})"
            tracks.append(T(title, ", ".join(filter(None, artists)), albums[0] if albums else None,
                            _iso_dur(a.get("duration")), a.get("isrc"), source_id=tid_))
    return {"name": attrs.get("name") or attrs.get("title") or "Tidal import", "description": attrs.get("description") or "",
            "cover": None, "tracks": tracks}


async def tidal(url: str, user_id: int | None = None) -> dict:
    m = re.search(r"tidal\.com/(?:browse/)?(playlist|album|track|mix)/([\w-]+)", url)
    if not m:
        raise SourceError("Not a Tidal playlist/album/track link")
    kind, tid = m.group(1), m.group(2)
    if kind == "mix":
        raise SourceError("Tidal mixes can't be read through the public API – save it as a playlist first")
    from . import connections
    tok = await connections.tidal_token(user_id) if user_id else None
    return await tidal_api(tok or await _tidal_token(), kind, tid)


# ---- Apple Music / Qobuz / any page with schema.org data -------------------------

def _walk_find(obj, pred):
    stack = [obj]
    while stack:
        o = stack.pop()
        if pred(o):
            return o
        if isinstance(o, dict):
            stack.extend(o.values())
        elif isinstance(o, list):
            stack.extend(o)
    return None


def _walk_all(obj, pred) -> list:
    out, stack = [], [obj]
    while stack:
        o = stack.pop()
        if pred(o):
            out.append(o)
        if isinstance(o, dict):
            stack.extend(reversed(list(o.values())))
        elif isinstance(o, list):
            stack.extend(reversed(o))
    return out


def _jsonld(html: str) -> dict | None:
    for m in re.finditer(r'<script[^>]+type="application/ld\+json"[^>]*>(.*?)</script>', html, re.S):
        try:
            data = json.loads(unescape(m.group(1)).strip())
        except ValueError:
            continue
        hit = _walk_find(data, lambda d: isinstance(d, dict) and d.get("@type") in ("MusicPlaylist", "MusicAlbum") and d.get("track"))
        if hit:
            return hit
    return None


def _ld_tracks(ld: dict) -> list[dict]:
    tr = ld.get("track")
    if isinstance(tr, dict):
        tr = tr.get("itemListElement") or [tr]
    out = []
    for t in tr or []:
        t = t.get("item", t) if isinstance(t, dict) else {}
        by = t.get("byArtist") or ld.get("byArtist") or {}
        if isinstance(by, list):
            by = {"name": ", ".join(b.get("name", "") for b in by)}
        dur = t.get("duration")
        out.append(T(t.get("name"), by.get("name"), (t.get("inAlbum") or {}).get("name"), _iso_dur(dur) if isinstance(dur, str) else None,
                     t.get("isrcCode")))
    return out


async def web_jsonld(url: str, user_id: int | None = None) -> dict:
    r = await client().get(url, headers=UA, follow_redirects=True)
    if r.status_code >= 400:
        raise SourceError(f"Page returned {r.status_code}")
    ld = _jsonld(r.text)
    if not ld:
        raise SourceError("No track list found on that page")
    img = ld.get("image")
    return {"name": ld.get("name") or "Imported playlist", "description": ld.get("description") or "",
            "cover": img if isinstance(img, str) else None, "tracks": _ld_tracks(ld)}


async def apple(url: str, user_id: int | None = None) -> dict:
    r = await client().get(url, headers=UA, follow_redirects=True)
    if r.status_code >= 400:
        raise SourceError(f"Apple Music returned {r.status_code}")
    ld = _jsonld(r.text)
    if ld and _ld_tracks(ld):
        return {"name": ld.get("name"), "description": ld.get("description") or "", "cover": None, "tracks": _ld_tracks(ld)}
    m = re.search(r'<script[^>]+id="serialized-server-data"[^>]*>(.*?)</script>', r.text, re.S)
    if not m:
        raise SourceError("Couldn't read the Apple Music page (is the playlist public?)")
    data = json.loads(m.group(1))
    rows = _walk_all(data, lambda d: isinstance(d, dict) and "artistName" in d and "title" in d and ("duration" in d or "contentDescriptor" in d))
    tracks = [T(x["title"], x["artistName"], x.get("tertiaryLinks", [{}])[0].get("title") if x.get("tertiaryLinks") else None,
                (x.get("duration") or 0) / 1000 or None) for x in rows]
    title = _walk_find(data, lambda d: isinstance(d, dict) and d.get("title") and "artwork" in d and "contentDescriptor" in d) or {}
    return {"name": title.get("title") or "Apple Music import", "description": "", "cover": None, "tracks": tracks}


# ---- ListenBrainz -----------------------------------------------------------------

async def listenbrainz(url: str, user_id: int | None = None) -> dict:
    m = re.search(r"playlist/([0-9a-f-]{36})", url)
    if not m:
        raise SourceError("Not a ListenBrainz playlist link")
    r = await client().get(f"https://api.listenbrainz.org/1/playlist/{m.group(1)}")
    if r.status_code >= 400:
        raise SourceError(f"ListenBrainz returned {r.status_code}")
    pl = r.json()["playlist"]
    tracks = []
    for t in pl.get("track", []):
        ids = t.get("identifier") or []
        ids = ids if isinstance(ids, list) else [ids]
        mbid = next((i.rsplit("/", 1)[-1] for i in ids if "musicbrainz.org/recording/" in i), None)
        tracks.append(T(t.get("title"), t.get("creator"), t.get("album"), (t.get("duration") or 0) / 1000 or None, mbid=mbid))
    return {"name": pl.get("title"), "description": pl.get("annotation") or "", "cover": None, "tracks": tracks}


# ---- YouTube Music (proper artist/album metadata, private playlists & Liked when connected) ----

async def ytmusic_fetch(url: str, user_id: int | None = None) -> dict:
    from . import ytmusic
    try:
        return await ytmusic.fetch(url, user_id)
    except ValueError:
        return await ytdlp_playlist(url, user_id)
    except RuntimeError as e:
        raise SourceError(str(e))


# ---- YouTube / SoundCloud / Bandcamp / … via yt-dlp ---------------------

async def ytdlp_playlist(url: str, user_id: int | None = None) -> dict:
    import yt_dlp

    def _go():
        with yt_dlp.YoutubeDL({"quiet": True, "no_warnings": True, "extract_flat": "in_playlist", "skip_download": True}) as y:
            return y.extract_info(url, download=False)
    try:
        info = await asyncio.to_thread(_go)
    except Exception as e:  # noqa: BLE001
        raise SourceError(f"Couldn't read that link: {str(e)[:200]}")
    entries = info.get("entries") or [info]
    tracks = []
    for e in entries:
        if not e:
            continue
        title = e.get("track") or e.get("title") or ""
        artist = e.get("artist") or e.get("creator") or ""
        channel = (e.get("channel") or e.get("uploader") or "").replace(" - Topic", "")
        if not artist:
            if " - " in title and not (e.get("channel") or "").endswith(" - Topic"):
                artist, title = title.split(" - ", 1)
            else:
                artist = channel
        title = re.sub(r"\s*[\(\[](official|lyric|audio|music video|visuali[sz]er|hd|hq|4k)[^\)\]]*[\)\]]", "", title, flags=re.I)
        tracks.append(T(title, artist, e.get("album"), e.get("duration"), source_id=e.get("url") or e.get("webpage_url") or e.get("id")))
    return {"name": info.get("title") or "Imported playlist", "description": info.get("description") or "",
            "cover": info.get("thumbnail"), "tracks": tracks}
